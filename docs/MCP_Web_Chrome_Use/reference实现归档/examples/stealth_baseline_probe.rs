//! 真机基线采集探针（临时工具，第 182/183 轮）。
//!
//! 用 laew 自己的启动参数集（`custom_launch_flags` + 视口/screen 策略，逐参数镜像
//! `browser.rs` 的 launch 路径）在本机真实 Chrome 上跑 `fingerprint::PROBE_JS`，
//! 把「只观测不回填」的向量钉上真机基线。
//!
//! 用法：
//!   cargo run --release --example stealth_baseline_probe -- headed
//!   cargo run --release --example stealth_baseline_probe -- hidden            # 新参数集(缺省,真 GPU)
//!   cargo run --release --example stealth_baseline_probe -- hidden softrender # 旧软渲染集(A/B 对照)
//!   cargo run --release --example stealth_baseline_probe -- compare           # 双档对账(第 183 轮验收)
//!
//! 输出 = PROBE_JS 的原始 JSON（一行）+ MQ4 + 服务端取证头 + 第 183 轮新向量，
//! 直接对账 `fingerprint::classify` 的判定项。

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::viewport::Viewport;
use futures::StreamExt;
use serde_json::Value;
use std::process::exit;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn chrome_exe() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("LAEW_BROWSER_PATH") {
        let pb = std::path::PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    for p in [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ] {
        let pb = std::path::PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    None
}

/// 第 183 轮:取证服务器 —— 把请求头**回灌进页面属性**,页面侧一次 eval 即可读到
/// 自己这次导航真实发出的 `Accept-Language`(免去服务端/客户端对账时序)。
async fn spawn_echo_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut socket, _)) => {
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
                        let pick = |name: &str| {
                            head.lines()
                                .find(|line| line.to_ascii_lowercase().starts_with(name))
                                .unwrap_or("(absent)")
                                .trim()
                                .to_string()
                        };
                        // 用属性承载头读数;做 HTML 转义防注入。
                        let esc = |s: String| s.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;");
                        let html = format!(
                            "<html><head><title>probe</title></head><body data-al=\"{}\" data-ch=\"{}\">probe</body></html>",
                            esc(pick("accept-language:")),
                            esc(pick("sec-ch-ua:"))
                        );
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

/// 第 183 轮新向量:滚动条宽度 / avail 几何 / DPR / connection / 媒体设备 /
/// 语音列表 / AudioContext 状态 / 本次导航真实发出的 Accept-Language 与 Sec-CH-UA。
const EXTRA_JS: &str = r#"(async () => {
  const scrollbar = window.innerWidth - document.documentElement.clientWidth;
  let media = null;
  try {
    const devs = await navigator.mediaDevices.enumerateDevices();
    media = devs.length;
  } catch (e) { media = 'err:' + (e && e.name); }
  let voices = null;
  try { voices = speechSynthesis.getVoices().length; } catch (e) { voices = null; }
  let audio = null;
  try { audio = (typeof AudioContext === 'function') ? new AudioContext().state : null; } catch (e) { audio = 'err'; }
  const conn = (() => {
    try {
      const c = navigator.connection;
      return c ? { et: c.effectiveType, rtt: c.rtt, dl: c.downlink, sd: c.saveData } : null;
    } catch (e) { return null; }
  })();
  return JSON.stringify({
    scrollbar_width: scrollbar,
    screen: screen.width + 'x' + screen.height,
    avail: screen.availWidth + 'x' + screen.availHeight + '@' + screen.availLeft + ',' + screen.availTop,
    dpr: window.devicePixelRatio,
    conn, media_devices: media, speech_voices: voices, audio_state: audio,
    accept_language: (document.body && document.body.dataset.al) || '',
    sec_ch_ua: (document.body && document.body.dataset.ch) || ''
  });
})()"#;

const MQ4_JS: &str = "JSON.stringify({hover:matchMedia('(hover:hover)').matches,\
 fine:matchMedia('(pointer:fine)').matches,\
 any_hover:matchMedia('(any-hover:hover)').matches,\
 any_fine:matchMedia('(any-pointer:fine)').matches})";

/// 第 184 轮候选向量:前 183 轮未覆盖的读数 —— OS 派生媒体特性族
/// (prefers-color-scheme/forced-colors/prefers-reduced-motion/prefers-contrast/
/// inverted-colors/color-gamut/dynamic-range/update/scripting)、显示器派生
/// (`screen.isExtended` 多显示器标记)、WebGPU 适配器(183 轮只对齐了 WebGL,
/// WebGPU 走独立适配路径)、Battery、canvas 渲染哈希(端到端验证 183 轮
/// 真 GPU 之后渲染产物是否双档一致)、`performance.memory` 堆上限。
const ROUND184_JS: &str = r#"(async () => {
  const mq = (q) => { try { return !!(window.matchMedia && matchMedia(q).matches); } catch (e) { return null; } };
  let webgpu = null;
  try {
    if (navigator.gpu) {
      const ad = await navigator.gpu.requestAdapter();
      if (ad) {
        let info = null;
        try {
          // Chrome 129+: GPUAdapter.info 同步 getter(requestAdapterInfo 已移除)
          const i = ad.info;
          info = i ? { vendor: i.vendor || '', architecture: i.architecture || '', device: i.device || '', description: i.description || '' } : null;
        } catch (e) { info = 'err'; }
        webgpu = { present: true, adapter: true, info };
      } else {
        webgpu = { present: true, adapter: false };
      }
    } else {
      webgpu = { present: false };
    }
  } catch (e) { webgpu = 'err:' + ((e && e.name) || ''); }
  let battery = null;
  try {
    if (navigator.getBattery) {
      const b = await navigator.getBattery();
      battery = { charging: b.charging, level: b.level, charging_time: b.chargingTime, discharging_time: b.dischargingTime };
    } else { battery = { absent: true }; }
  } catch (e) { battery = 'err'; }
  let canvas_hash = null;
  try {
    const c = document.createElement('canvas'); c.width = 220; c.height = 30;
    const g = c.getContext('2d');
    g.textBaseline = 'top'; g.font = '14px Arial'; g.fillStyle = '#f60';
    g.fillRect(0, 0, 120, 30); g.fillStyle = '#069';
    g.fillText('BrowserLeaks,com <canvas> 1.0', 2, 15);
    g.fillStyle = 'rgba(102,204,0,0.7)';
    g.fillText('BrowserLeaks,com <canvas> 1.0', 4, 17);
    const url = c.toDataURL();
    let h = 5381;
    for (let i = 0; i < url.length; i++) { h = ((h << 5) + h + url.charCodeAt(i)) | 0; }
    canvas_hash = url.length + ':' + (h >>> 0).toString(16);
  } catch (e) { canvas_hash = 'err'; }
  return JSON.stringify({
    mq: {
      cs_dark: mq('(prefers-color-scheme: dark)'),
      cs_light: mq('(prefers-color-scheme: light)'),
      forced_active: mq('(forced-colors: active)'),
      forced_none: mq('(forced-colors: none)'),
      reduced_motion: mq('(prefers-reduced-motion: reduce)'),
      reduced_motion_nopref: mq('(prefers-reduced-motion: no-preference)'),
      contrast_more: mq('(prefers-contrast: more)'),
      inverted: mq('(inverted-colors: inverted)'),
      gamut_p3: mq('(color-gamut: p3)'),
      gamut_rec2020: mq('(color-gamut: rec2020)'),
      dynamic_high: mq('(dynamic-range: high)'),
      dynamic_standard: mq('(dynamic-range: standard)'),
      video_hdr: mq('(video-dynamic-range: high)'),
      update_fast: mq('(update: fast)'),
      scripting_enabled: mq('(scripting: enabled)')
    },
    screen_is_extended: (typeof screen.isExtended === 'boolean') ? screen.isExtended : null,
    webgpu, battery, canvas_hash,
    heap_limit: (performance.memory && performance.memory.jsHeapSizeLimit) || null
  });
})()"#;

/// 单档采集:镜像生产 launch 路径(参数集/视口/screen_sync/指针仿真),返回
/// (PROBE, MQ4, EXTRA, R184) 四个已解析 JSON。
async fn run_once(mode: &str, softrender: bool) -> (Value, Value, Value, Value) {
    let headless = mode != "headed";
    let Some(exe) = chrome_exe() else {
        eprintln!("未找到 Chrome");
        exit(2);
    };
    let mut builder = BrowserConfig::builder()
        .chrome_executable(exe)
        .disable_default_args();
    // 镜像 custom_launch_flags(headless=true, geo_prompt=false;第 183 轮起无头档
    // 默认真值参数集 —— 不带 disable-gpu/hide-scrollbars,softrender=on 恢复旧集)。
    for f in [
        "no-first-run",
        "no-default-browser-check",
        "disable-background-mode",
        "no-service-autorun",
        "disable-component-update",
        "hide-crash-restore-bubble",
        "disable-session-crashed-bubble",
        "disable-background-networking",
        "disable-background-timer-throttling",
        "disable-backgrounding-occluded-windows",
        "disable-component-extensions-with-background-pages",
        "disable-default-apps",
        "disable-hang-monitor",
        "disable-ipc-flooding-protection",
        "disable-prompt-on-repost",
        "disable-renderer-backgrounding",
        "disable-sync",
        "disable-popup-blocking",
        "metrics-recording-only",
        "ignore-certificate-errors",
        "disable-blink-features=AutomationControlled",
        "deny-permission-prompts",
    ] {
        builder = builder.arg(f);
    }
    if headless {
        let mut extra: Vec<&str> = vec!["no-sandbox", "disable-dev-shm-usage"];
        if softrender {
            extra.extend(["disable-gpu", "hide-scrollbars"]);
        }
        for f in extra {
            builder = builder.arg(f);
        }
        builder = builder.new_headless_mode();
    } else {
        builder = builder.with_head();
    }
    builder = builder.window_size(1920, 1080);
    // 镜像第 161 轮视口策略：headed 禁用视口仿真；headless 显式 1920×1080。
    builder = if headless {
        builder.viewport(Viewport {
            width: 1920,
            height: 1080,
            device_scale_factor: None,
            emulating_mobile: false,
            is_landscape: false,
            has_touch: false,
        })
    } else {
        builder.viewport(None::<Viewport>)
    };
    let config = builder.build().expect("BrowserConfig");

    let (mut browser, mut handler) = Browser::launch(config).await.expect("launch");
    let _handle = tokio::spawn(async move { while let Some(_ev) = handler.next().await {} });
    let url = spawn_echo_server().await;
    let parse = |text: Option<String>| serde_json::from_str::<Value>(&text.unwrap_or_default()).unwrap_or(Value::Null);
    let page = browser.new_page("about:blank").await.expect("new_page");
    // 镜像第 175 轮 screen_sync + 第 182 轮指针/触摸仿真(生产函数)。
    if headless {
        let params =
            chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams::builder()
                .width(1920)
                .height(1080)
                .device_scale_factor(1.0)
                .screen_width(1920)
                .screen_height(1080)
                .position_x(0)
                .position_y(0)
                .mobile(false)
                .build()
                .expect("params");
        page.execute(params).await.expect("screen_sync");
        lsm_agent::agent::browser_env_emulation::apply_pointer_env_emulation(
            &page,
            lsm_agent::agent::stealth::StealthMode::Hardened,
            lsm_agent::agent::browser_mode::BrowserMode::NewHeadless,
        )
        .await;
    }
    page.goto(url.as_str()).await.expect("goto");
    // 语音列表异步填充 + 首文档语言列表需要一瞬(与真实 open 路径同节奏)。
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let probe = parse(
        page.evaluate(lsm_agent::agent::fingerprint::PROBE_JS)
            .await
            .expect("probe")
            .into_value::<String>()
            .ok(),
    );
    let mq = parse(
        page.evaluate(MQ4_JS).await.expect("mq").into_value::<String>().ok(),
    );
    let extra = parse(
        page.evaluate(EXTRA_JS).await.expect("extra").into_value::<String>().ok(),
    );
    let r184 = parse(
        page.evaluate(ROUND184_JS).await.expect("r184").into_value::<String>().ok(),
    );
    let _ = browser.close().await;
    (probe, mq, extra, r184)
}

/// 第 184 轮核心实证:子帧 realm 覆盖面 + 跨 realm toString 反查。
///
/// 疑点(源码层已证实):chromiumoxide 0.9.1 的 `AddScriptToEvaluateOnNewDocument`
/// 无 `runForEveryFrame` 字段(CDP 缺省 false = 脚本只进顶层 frame)。若属实:
/// ① iframe 内 `navigator.webdriver` 读裸值(未被 CORE 覆写);
/// ② 站点自建同源 iframe 的 **原生** `Function.prototype.toString` 反查顶层被改写
///    函数(toString 自身 / webdriver getter / permissions.query)即读到我们的源码
///    —— 击穿侵入式档「改过不可反查」。
const CROSSREALM_JS: &str = r#"(async () => {
  const f = document.createElement('iframe');
  f.setAttribute('srcdoc', '<html><body>inner</body></html>');
  f.style.display = 'none';
  const loaded = new Promise((res) => { f.onload = () => res(1); });
  document.body.appendChild(f);
  await Promise.race([loaded, new Promise((r) => setTimeout(() => r(0), 1500))]);
  const out = { frame_ready: !!(f.contentWindow && f.contentWindow.Function) };
  if (out.frame_ready) {
    const W = f.contentWindow;
    try { out.iframe_webdriver = W.navigator.webdriver; } catch (e) { out.iframe_webdriver = 'err'; }
    try { out.iframe_plugins = W.navigator.plugins ? W.navigator.plugins.length : null; } catch (e) {}
    // 内层 toString 是否被我们处置(= 隐身脚本进没进子帧)。**判据必须用跨 realm
    // 反查推导**:自反调用在「未处置(原生)」与「已处置(自登记)」两种情况下都返回
    // [native code],无判别力;用内层 toString 反查外层被改写的 toString 才分得开
    // —— 原生 → 吐我们的源码,已处置+路由 → 报注册串。
    try {
      out.iframe_ts_patched = /\[native code\]/.test(out.leak_ts_via_iframe = String(W.Function.prototype.toString.call(Function.prototype.toString)));
    } catch (e) { out.iframe_ts_patched = 'err'; }
    // 攻击 1:内层 toString 反查外层被改写的 toString 自身
    try { out.leak_ts_via_iframe = String(W.Function.prototype.toString.call(Function.prototype.toString)); } catch (e) { out.leak_ts_via_iframe = 'err'; }
    // 攻击 2:反查外层 webdriver getter(CORE 恒改写)
    try {
      const d = Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver');
      out.leak_webdriver_getter = (d && d.get) ? String(W.Function.prototype.toString.call(d.get)) : null;
    } catch (e) { out.leak_webdriver_getter = 'err'; }
    // 攻击 3:反查 permissions.query(FIDELITY ⑤ 改写)
    try { out.leak_query = String(W.Function.prototype.toString.call(Permissions.prototype.query)); } catch (e) { out.leak_query = 'err'; }
  }
  // 顶层基线(hardened 下应全 native)
  try { out.top_ts_self = String(Function.prototype.toString.call(Function.prototype.toString)); } catch (e) { out.top_ts_self = 'err'; }
  try { out.top_webdriver = navigator.webdriver; } catch (e) { out.top_webdriver = 'err'; }
  return JSON.stringify(out);
})()"#;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("crossrealm") {
        // 第 184 轮:镜像生产注入(stealth_js_for(Hardened) 经 addScriptToEvaluateOnNewDocument
        // + 导航),验证子帧覆盖面与跨 realm 反查。
        use lsm_agent::agent::browser_ns::inject_stealth;
        let Some(exe) = chrome_exe() else { eprintln!("未找到 Chrome"); exit(2); };
        let mut builder = BrowserConfig::builder()
            .chrome_executable(exe)
            .disable_default_args();
        for f in ["no-first-run", "no-default-browser-check", "disable-background-mode",
            "no-service-autorun", "disable-component-update", "hide-crash-restore-bubble",
            "disable-session-crashed-bubble", "disable-background-networking",
            "disable-background-timer-throttling", "disable-backgrounding-occluded-windows",
            "disable-component-extensions-with-background-pages", "disable-default-apps",
            "disable-hang-monitor", "disable-ipc-flooding-protection", "disable-prompt-on-repost",
            "disable-renderer-backgrounding", "disable-sync", "disable-popup-blocking",
            "metrics-recording-only", "ignore-certificate-errors",
            "disable-blink-features=AutomationControlled", "deny-permission-prompts"] {
            builder = builder.arg(f);
        }
        builder = builder
            .arg("no-sandbox")
            .arg("disable-dev-shm-usage")
            .new_headless_mode()
            .window_size(1920, 1080)
            .viewport(Viewport {
                width: 1920, height: 1080, device_scale_factor: None,
                emulating_mobile: false, is_landscape: false, has_touch: false,
            });
        let (mut browser, mut handler) = Browser::launch(builder.build().expect("cfg")).await.expect("launch");
        tokio::spawn(async move { while let Some(_ev) = handler.next().await {} });
        let url = spawn_echo_server().await;
        let page = browser.new_page("about:blank").await.expect("new_page");
        inject_stealth(&page, lsm_agent::agent::stealth::StealthMode::Hardened).await;
        page.goto(url.as_str()).await.expect("goto");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let raw = page.evaluate(CROSSREALM_JS).await.expect("eval").into_value::<String>().unwrap_or_default();
        println!("CROSSREALM: {raw}");
        // 生产 PROBE_JS 的同步路径验证(创建 iframe 与测量在**同一次同步 eval** 内,
        // 测的是初始 about:blank realm —— 时序最紧的真实攻击面):修复后
        // crossrealm_tostring_leak 应为空数组、screen_is_extended.md_marker 应为 true
        // (多显示器机器)且 value 经 FIDELITY_JS ⑥ 回填为 true。
        let probe = page.evaluate(lsm_agent::agent::fingerprint::PROBE_JS)
            .await.expect("probe").into_value::<String>().unwrap_or_default();
        let parsed: Value = serde_json::from_str(&probe).unwrap_or(Value::Null);
        println!("PROBE_CROSSREALM_FIELD: {}", serde_json::to_string(
            &serde_json::json!({
                "crossrealm_tostring_leak": parsed.get("crossrealm_tostring_leak"),
                "screen_is_extended": parsed.get("screen_is_extended"),
                "injected_ns_symbols": parsed.get("injected_ns_symbols"),
                "patched_fn_leak": parsed.get("patched_fn_leak")
            })
        ).unwrap_or_default());
        let _ = browser.close().await;
        exit(0);
    }
    if args.first().map(String::as_str) == Some("compare") {
        // 第 183 轮验收载体:同进程先后跑有头/无头双档,核心向量逐项 PASS/FAIL 对账。
        let (h_probe, h_mq, h_extra, h_r184) = run_once("headed", false).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let (n_probe, n_mq, n_extra, n_r184) = run_once("hidden", false).await;
        println!("HEADED  PROBE: {h_probe}");
        println!("HEADED  EXTRA: {h_extra}");
        println!("HEADED  R184: {h_r184}");
        println!("HEADLESS PROBE: {n_probe}");
        println!("HEADLESS EXTRA: {n_extra}");
        println!("HEADLESS R184: {n_r184}");
        let mut fails = 0usize;
        let mut check = |name: &str, ok: bool, detail: String| {
            println!("[{}] {name}: {detail}", if ok { "PASS" } else { "FAIL" });
            if !ok {
                fails += 1;
            }
        };
        // ① WebGL 双串逐字节一致(第 183 轮核心:无头档真 GPU)
        let hv = h_probe["webgl"]["vendor"].clone();
        let nv = n_probe["webgl"]["vendor"].clone();
        let hr = h_probe["webgl"]["renderer"].clone();
        let nr = n_probe["webgl"]["renderer"].clone();
        check(
            "webgl_vendor",
            hv == nv && !hv.is_null(),
            format!("headed={hv} headless={nv}"),
        );
        check(
            "webgl_renderer",
            hr == nr && !hr.is_null(),
            format!("headed={hr} headless={nr}"),
        );
        // ② 滚动条宽度一致(跟随宿主 OS 默认)
        check(
            "scrollbar_width",
            h_extra["scrollbar_width"] == n_extra["scrollbar_width"],
            format!("headed={} headless={}", h_extra["scrollbar_width"], n_extra["scrollbar_width"]),
        );
        // ③ 指针四元 + 触摸点双档一致(第 182 轮仿真后无头应与有头同为全 true / 0)
        let mq_ok = |m: &Value| {
            ["hover", "fine", "any_hover", "any_fine"]
                .iter()
                .all(|k| m[*k] == Value::Bool(true))
        };
        check("pointer_mq4", mq_ok(&h_mq) && mq_ok(&n_mq), format!("headed={h_mq} headless={n_mq}"));
        check(
            "max_touch_points",
            h_probe["max_touch_points"] == n_probe["max_touch_points"]
                && h_probe["max_touch_points"] == Value::from(0u64),
            format!("headed={} headless={}", h_probe["max_touch_points"], n_probe["max_touch_points"]),
        );
        // ④ Accept-Language 一致(Chrome 全新 profile 从 OS 种子,双档同值)
        check(
            "accept_language",
            h_extra["accept_language"] == n_extra["accept_language"]
                && !h_extra["accept_language"].as_str().unwrap_or("").is_empty(),
            format!("headed={} headless={}", h_extra["accept_language"], n_extra["accept_language"]),
        );
        // ⑤ 插件数一致(new headless 原生即 5)
        check(
            "plugins",
            h_probe["plugins"] == n_probe["plugins"] && h_probe["plugins"] != Value::from(0u64),
            format!("headed={} headless={}", h_probe["plugins"], n_probe["plugins"]),
        );
        // ⑥ 第 184 轮新对账项(38 号文 §二基线的代码级验收):
        //    canvas 渲染哈希(37 轮真 GPU 的端到端实证)/ WebGPU 适配器 / color-scheme
        //    (无头档跟随 OS)。纯环境层对账,与注入无关(compare 不注入隐身脚本)。
        check(
            "canvas_render_hash",
            h_r184["canvas_hash"] == n_r184["canvas_hash"]
                && !h_r184["canvas_hash"].as_str().unwrap_or("").is_empty()
                && !h_r184["canvas_hash"].as_str().unwrap_or("").starts_with("err"),
            format!("headed={} headless={}", h_r184["canvas_hash"], n_r184["canvas_hash"]),
        );
        check(
            "webgpu_adapter",
            h_r184["webgpu"] == n_r184["webgpu"] && h_r184["webgpu"]["adapter"] == Value::Bool(true),
            format!("headed={} headless={}", h_r184["webgpu"], n_r184["webgpu"]),
        );
        check(
            "prefers_color_scheme",
            h_r184["mq"]["cs_dark"] == n_r184["mq"]["cs_dark"]
                && !h_r184["mq"]["cs_dark"].is_null(),
            format!("headed_cs_dark={} headless_cs_dark={}", h_r184["mq"]["cs_dark"], n_r184["mq"]["cs_dark"]),
        );
        if fails == 0 {
            println!("compare: 全项 PASS");
        } else {
            println!("compare: {fails} 项 FAIL");
            exit(1);
        }
        return;
    }
    let mode = args.first().cloned().unwrap_or_else(|| "hidden".into());
    let softrender = args.iter().any(|a| a == "softrender");
    let (probe, mq, extra, r184) = run_once(&mode, softrender).await;
    println!("PROBE: {probe}");
    println!("MQ4: {mq}");
    println!("EXTRA: {extra}");
    println!("R184: {r184}");
    exit(0);
}
