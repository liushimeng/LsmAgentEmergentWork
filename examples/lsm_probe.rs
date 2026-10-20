//! 临时探针:直接驱动 LsmAgentBrowser 内核,复现/验证布局与快照桥行为。
//!
//! 用法:`cargo run --example lsm_probe -- <url> [selector]`
//! 仅用于开发期诊断,不属于产品能力。

use lsm_agent::agent::browser::{self, BrowserMode, LsmBrowserManager};
use serde_json::json;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| "https://map.baidu.com/".to_string());
    let selector = args.next().unwrap_or_else(|| "input".to_string());

    let mgr = LsmBrowserManager::new();
    let outcome = mgr
        .new_page(&url, BrowserMode::Hidden, None, Some(1280), Some(720), false, false, 60_000)
        .await
        .expect("open failed");
    let pid = outcome.page_id.clone();
    println!("== open ==");
    println!("page_id={} title={} status={} bytes={}", pid, outcome.title, outcome.status, outcome.source_bytes);

    let params = json!({"selector": selector, "include_style": true});
    match browser::inspect::inspect(&mgr, &pid, "elements", &params).await {
        Ok(v) => println!("== elements({selector}) ==\n{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => println!("elements error: {} {}", e.code, e.message),
    }

    // 取页面内联脚本源码(定位 JS 运行时失败点)
    match browser::inspect::inspect(&mgr, &pid, "dom", &json!({"selector": "html"})).await {
        Ok(v) => {
            let raw = serde_json::to_string(&v).unwrap_or_default();
            if let Some(i) = raw.find("mergedRequire") {
                let lo = i.saturating_sub(200);
                println!("== requirejs source ==\n{}", &raw[lo..(lo + 12000).min(raw.len())]);
            } else {
                println!("== dom raw len={} (no mergedRequire) ==", raw.len());
            }
            if let Some(arr) = v.get("tree").and_then(|t| t.get("children")).and_then(|c| c.as_array()) {
                for (i, c) in arr.iter().enumerate() {
                    let txt = c.get("children").and_then(|x| x.as_array()).map(|cs| {
                        cs.iter().filter_map(|n| n.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("")
                    }).unwrap_or_default();
                    if txt.contains("requirejs") {
                        println!("== inline script #{i} ({} chars) ==\n{}", txt.len(), &txt[..txt.len().min(6000)]);
                    }
                }
            }
        }
        Err(e) => println!("dom error: {} {}", e.code, e.message),
    }

    for info in ["viewport", "page_health", "text", "console"] {
        match browser::inspect::inspect(&mgr, &pid, info, &json!({})).await {
            Ok(v) => {
                let s = serde_json::to_string(&v).unwrap_or_default();
                println!("== {info} ==\n{}", &s[..s.len().min(2000)]);
            }
            Err(e) => println!("{info} error: {} {}", e.code, e.message),
        }
    }

    // eval_js:验证快照桥能否读到元素文本
    let js = std::env::var("LAEW_PROBE_JS").unwrap_or_else(|_| "(() => { const out=[]; document.querySelectorAll('a,button,li,span,div,i,em,input').forEach(el=>{ const t=(el.innerText||el.textContent||el.title||'').trim(); if(t) out.push({tag:el.tagName, cls:String(el.className).slice(0,40), text:t.slice(0,30)}); }); return JSON.stringify(out.slice(0,20)); })()".to_string());
    let top = json!({"action": "control", "control_action": "eval_js", "page_id": pid, "params": {"js": js}});
    match browser::api::control(&mgr, Some(&pid), "eval_js", &json!({"js": js}), &top).await {
        Ok(v) => println!("== eval_js ==\n{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => println!("eval_js error: {} {}", e.code, e.message),
    }

    mgr.shutdown().await;
}
