//! 单脚本执行耗时探针(离线二分用,不属于产品能力)。
//!
//! 用法:`cargo run --example js_bench -- <脚本文件> [来源URL] [预算毫秒]`
//!
//! 用途:把真站某个重脚本单独喂给页面 JS 线程,量出墙钟耗时;配合
//! `LAEW_JS_FORIN_GUARD=off` / `LAEW_AMD_FILL=off` 等开关做 A/B 对比,
//! 定位「管线预算被谁吃掉」而不必反复打开真站。

use std::time::Instant;

use lsm_agent::agent::browser::script::{JsHandle, JsRequest, PageEnv, ScriptSource};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("需要脚本文件路径");
    let src_url = args
        .next()
        .unwrap_or_else(|| "https://www.jd.com/bench.js".to_string());
    let budget_ms: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(600_000);

    let content = std::fs::read_to_string(&path).expect("读脚本失败");
    println!("script={path} bytes={} url={src_url}", content.len());

    let env = PageEnv {
        url: "https://www.jd.com/".to_string(),
        title: "bench".to_string(),
        cookies: String::new(),
        local_storage: Default::default(),
        session_storage: Default::default(),
        viewport_width: 1920,
        viewport_height: 1080,
        scroll_x: 0.0,
        scroll_y: 0.0,
        user_agent: lsm_agent::agent::browser::DEFAULT_UA.to_string(),
        prefers_dark: false,
        csp: None,
        resources: Vec::new(),
    };

    let handle = JsHandle::spawn();
    let started = Instant::now();
    let reply = tokio::task::spawn_blocking(move || {
        handle.request_with_budget(
            |tx| JsRequest::RunScripts {
                scripts: vec![ScriptSource {
                    kind: "external",
                    content,
                    src_url,
                }],
                env,
                nodes: Vec::new(),
                reply: tx,
                budget_ms,
            },
            budget_ms,
        )
    })
    .await
    .expect("blocking join 失败");

    match reply {
        Ok(r) => {
            println!(
                "== done in {} ms == ok={} error={:?} actions={} logs={}",
                started.elapsed().as_millis(),
                r.outcome.ok,
                r.outcome.error,
                r.outcome.actions.len(),
                r.outcome.logs.len()
            );
            for l in r.outcome.logs.iter().take(20) {
                println!("[{}] {}", l.level, l.text);
            }
        }
        Err(e) => println!("== timeout/fail after {} ms: {e} ==", started.elapsed().as_millis()),
    }
}
