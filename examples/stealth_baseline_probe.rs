//! 真机基线采集探针（临时工具，第 182 轮）。
//!
//! 用 laew 自己的启动参数集（`custom_launch_flags` + 视口/screen 策略，逐参数镜像
//! `browser.rs` 的 launch 路径）在本机真实 Chrome 上跑 `fingerprint::PROBE_JS`，
//! 把第 35 轮「只观测不回填」的向量钉上真机基线。
//!
//! 用法：
//!   cargo run --release --example stealth_baseline_probe -- headed
//!   cargo run --release --example stealth_baseline_probe -- hidden
//!
//! 输出 = PROBE_JS 的原始 JSON（一行），直接对账 `fingerprint::classify` 的判定项。

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::viewport::Viewport;
use futures::StreamExt;
use std::process::exit;

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

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "hidden".into());
    let headless = mode != "headed";
    let Some(exe) = chrome_exe() else {
        eprintln!("未找到 Chrome");
        exit(2);
    };

    let mut builder = BrowserConfig::builder()
        .chrome_executable(exe)
        .disable_default_args();
    // 镜像 custom_launch_flags(headless=true, geo_prompt=false)。
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
        for f in ["disable-gpu", "no-sandbox", "disable-dev-shm-usage", "hide-scrollbars"] {
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

    let page = browser.new_page("about:blank").await.expect("new_page");

    // 镜像第 175 轮 screen_sync：headless 档补 screen_width/screen_height 声明。
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
        // 第 182 轮:走**生产函数**做指针/触摸环境仿真,探针读数即对账依据。
        lsm_agent::agent::browser_env_emulation::apply_pointer_env_emulation(
            &page,
            lsm_agent::agent::stealth::StealthMode::Hardened,
            lsm_agent::agent::browser_mode::BrowserMode::NewHeadless,
        )
        .await;
    }

    // 预热（全新 profile 首文档语言列表等需要一瞬），与真实 open 路径同节奏。
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let out = page
        .evaluate(lsm_agent::agent::fingerprint::PROBE_JS)
        .await
        .expect("evaluate");
    println!("{}", out.into_value::<String>().unwrap_or_default());
    // 四元媒体查询(PROBE_JS 只采 hover/fine 二元,any-* 在此补采)
    let mq = page
        .evaluate(
            "JSON.stringify({hover:matchMedia('(hover:hover)').matches,\
             fine:matchMedia('(pointer:fine)').matches,\
             any_hover:matchMedia('(any-hover:hover)').matches,\
             any_fine:matchMedia('(any-pointer:fine)').matches})",
        )
        .await
        .expect("mq");
    println!("MQ4: {}", mq.into_value::<String>().unwrap_or_default());

    let _ = browser.close().await;
    exit(0);
}
