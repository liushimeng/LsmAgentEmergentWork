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

/// 单档采集:镜像生产 launch 路径(参数集/视口/screen_sync/指针仿真),返回
/// (PROBE, MQ4, EXTRA) 三个已解析 JSON。
async fn run_once(mode: &str, softrender: bool) -> (Value, Value, Value) {
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
    let _ = browser.close().await;
    (probe, mq, extra)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("compare") {
        // 第 183 轮验收载体:同进程先后跑有头/无头双档,核心向量逐项 PASS/FAIL 对账。
        let (h_probe, h_mq, h_extra) = run_once("headed", false).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let (n_probe, n_mq, n_extra) = run_once("hidden", false).await;
        println!("HEADED  PROBE: {h_probe}");
        println!("HEADED  EXTRA: {h_extra}");
        println!("HEADLESS PROBE: {n_probe}");
        println!("HEADLESS EXTRA: {n_extra}");
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
    let (probe, mq, extra) = run_once(&mode, softrender).await;
    println!("PROBE: {probe}");
    println!("MQ4: {mq}");
    println!("EXTRA: {extra}");
    exit(0);
}
