//! 临时探针:直接驱动 LsmAgentBrowser 内核,复现/验证布局与快照桥行为。
//!
//! 用法:`cargo run --example lsm_probe -- <url> [selector] [width] [height] [outdir]`
//! 仅用于开发期诊断,不属于产品能力。

use lsm_agent::agent::browser::{self, BrowserMode, LsmBrowserManager};
use serde_json::json;

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…(len={})", &s[..end], s.len())
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| "https://map.baidu.com/".to_string());
    let selector = args.next().unwrap_or_else(|| "input".to_string());
    let width: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1920);
    let height: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1080);
    let outdir = args.next().unwrap_or_else(|| "tmpPlan/probe_jd".to_string());

    let started = std::time::Instant::now();
    let mgr = LsmBrowserManager::new();
    let outcome = mgr
        .new_page(
            &url,
            BrowserMode::Hidden,
            None,
            Some(width),
            Some(height),
            false,
            false,
            60_000,
        )
        .await
        .expect("open failed");
    let pid = outcome.page_id.clone();
    println!("== open ({} ms) ==", started.elapsed().as_millis());
    println!(
        "page_id={} title={} status={} bytes={}",
        pid, outcome.title, outcome.status, outcome.source_bytes
    );

    // LAEW_PROBE_FAST=1:跳过截图/元素/布局类观察(debug 档下整页光栅化与
    // 大 DOM 重排可达分钟级),只取 console/network 台账定位脚本失败根因。
    let fast = std::env::var("LAEW_PROBE_FAST")
        .map(|v| !matches!(v.trim(), "0" | "off" | "false" | "no"))
        .unwrap_or(false);
    // 截图落盘,直接看用户观感
    let shot = format!("{outdir}/jd_shot.png");
    let top = json!({"action": "control", "control_action": "screenshot", "page_id": pid,
                     "params": {"save_path": shot, "full_page": true}});
    if !fast {
        match browser::api::control(&mgr, Some(&pid), "screenshot", &json!({"save_path": shot, "full_page": true}), &top).await {
            Ok(v) => println!("== screenshot ==\n{}", clip(&serde_json::to_string(&v).unwrap_or_default(), 600)),
            Err(e) => println!("screenshot error: {} {}", e.code, e.message),
        }

        let params = json!({"selector": selector, "include_style": true});
        match browser::inspect::inspect(&mgr, &pid, "elements", &params).await {
            Ok(v) => println!("== elements({selector}) ==\n{}", clip(&serde_json::to_string_pretty(&v).unwrap_or_default(), 6000)),
            Err(e) => println!("elements error: {} {}", e.code, e.message),
        }
    }

    let infos: &[&str] = if fast {
        &["console", "network"]
    } else {
        &["viewport", "page_health", "text", "console", "network"]
    };
    for info in infos {
        let info = *info;
        match browser::inspect::inspect(&mgr, &pid, info, &json!({})).await {
            Ok(v) => {
                let s = serde_json::to_string_pretty(&v).unwrap_or_default();
                let limit = if info == "console" || info == "network" { 60_000 } else { 3000 };
                println!("== {info} ==\n{}", clip(&s, limit));
            }
            Err(e) => println!("{info} error: {} {}", e.code, e.message),
        }
    }

    // eval_js:验证快照桥能否读到元素文本
    if fast {
        mgr.shutdown().await;
        println!("== done in {} ms ==", started.elapsed().as_millis());
        return;
    }
    let js = std::env::var("LAEW_PROBE_JS").unwrap_or_else(|_| "(() => { const out=[]; document.querySelectorAll('a,button,li,span,div,i,em,input').forEach(el=>{ const t=(el.innerText||el.textContent||el.title||'').trim(); if(t) out.push({tag:el.tagName, cls:String(el.className).slice(0,40), text:t.slice(0,30)}); }); return JSON.stringify(out.slice(0,20)); })()".to_string());
    let top = json!({"action": "control", "control_action": "eval_js", "page_id": pid, "params": {"js": js}});
    match browser::api::control(&mgr, Some(&pid), "eval_js", &json!({"js": js}), &top).await {
        Ok(v) => println!("== eval_js ==\n{}", clip(&serde_json::to_string_pretty(&v).unwrap_or_default(), 8000)),
        Err(e) => println!("eval_js error: {} {}", e.code, e.message),
    }

    mgr.shutdown().await;
    println!("== done in {} ms ==", started.elapsed().as_millis());
}
