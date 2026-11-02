//! 临时探针:复现「查看器白板 vs 引擎截图有内容」的显示层断链。
//!
//! 用法:`cargo run --example lsm_view_probe -- [outdir]`
//! 阶段:
//!   A. 本地 loopback 彩色页(红/绿/蓝色块+黑字) headed 打开 → 抓全屏 + 引擎截图;
//!   B. 再开一个 about:blank 小页(前台抢占) → 抓全屏,验证前台切换行为;
//!   C. 对彩色页跑一次 eval_js 补泵(交互路径重排) → 再抓全屏。
//! 比较三张全屏里的查看器窗口与引擎截图即可定位断点在哪一层。
//! 仅开发期诊断,不属于产品能力。

use lsm_agent::agent::browser::{self, api, inspect, BrowserMode, LsmBrowserManager};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;

fn clip(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    if s.len() > 300 {
        format!("{}…", &s[..300])
    } else {
        s
    }
}

/// 极简 loopback HTTP 服务:接受一个连接,回一个彩色页,保持监听不关。
fn spawn_color_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = concat!(
                "<!doctype html><html><head><meta charset='utf-8'><title>ProbeColor</title></head>",
                "<body style='margin:0'>",
                "<div style='height:240px;background:#e60012'></div>",
                "<div style='height:240px;background:#00a651'></div>",
                "<div style='height:240px;background:#0057ff'></div>",
                "<div style='height:240px;background:#ffd400'></div>",
                "<div style='padding:40px;font-size:64px;color:#000'>PROBE-VIEWER-OK 12345</div>",
                "</body></html>"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

fn screencap(path: &str) {
    let st = std::process::Command::new("screencapture")
        .arg("-x")
        .arg(path)
        .status();
    match st {
        Ok(st) => println!("[probe] screencapture {path} -> {:?}", st.code()),
        Err(e) => println!("[probe] screencapture {path} 失败: {e}"),
    }
}

/// 列出当前查看器子进程(验证是否被级联重生)。
fn list_viewers(tag: &str) {
    let out = std::process::Command::new("pgrep")
        .args(["-fl", "browser-view"])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout).trim_end().to_string();
            println!("[probe] viewers@{tag}:\n{s}");
        }
        Err(e) => println!("[probe] viewers@{tag} pgrep 失败: {e}"),
    }
}

async fn engine_shot(mgr: &LsmBrowserManager, pid: &str, path: &str) {
    let top = json!({"action":"control","control_action":"screenshot","page_id":pid,
                     "params":{"save_path":path}});
    match api::control(mgr, Some(pid), "screenshot", &json!({"save_path": path}), &top).await {
        Ok(v) => println!("[probe] engine screenshot {path}: {}", clip(&v)),
        Err(e) => println!("[probe] engine screenshot error: {} {}", e.code, e.message),
    }
}

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tmpPlan/probe_viewer".to_string());
    std::fs::create_dir_all(&outdir).unwrap();
    // loopback 需要 SSRF 放行(测试同款)。
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    std::env::set_var("LAEW_BROWSER_VIEW_ACTIVATE", "off");
    // 关键:查看器默认拉 current_exe() —— 探针二进制会把 `__browser-view`
    // 当普通参数再跑一遍 main() 造成递归 spawn。必须指向真正的 laew。
    let laew_bin = std::env::var("LAEW_BIN")
        .unwrap_or_else(|_| "/Users/dev02/Desktop/MyLocalGit/LsmAgentEmergentWork/laew".to_string());
    std::env::set_var("LAEW_BROWSER_VIEWER_BIN", &laew_bin);
    println!("[probe] viewer bin = {laew_bin}");

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        let port = spawn_color_server();
        let url = format!("http://127.0.0.1:{port}/probe.html");
        println!("[probe] color page: {url}");

        // 用全局单例(new_arc):与真实工具链路一致,1Hz 心跳会跑。
        let mgr = LsmBrowserManager::global();
        let t0 = std::time::Instant::now();
        let out = mgr
            .new_page(
                &url,
                BrowserMode::Headed,
                None,
                Some(1920),
                Some(1080),
                false,
                false,
                30_000,
                &browser::NavHints::default(),
            )
            .await
            .expect("open color page failed");
        let pid = out.page_id.clone();
        println!(
            "[probe] A open done {}ms page_id={} title={}",
            t0.elapsed().as_millis(),
            pid,
            out.title
        );

        // 等 ticker(1Hz) + 查看器 tick(100ms) 吃到帧。
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        list_viewers("A");
        screencap(&format!("{outdir}/screen_A_color.png"));
        engine_shot(&mgr, &pid, &format!("{outdir}/engine_A_color.png")).await;

        if let Ok(v) = inspect::inspect(&mgr, &pid, "page_health", &json!({})).await {
            println!(
                "[probe] A health: layout_boxes={:?} visible_boxes={:?} visible_text_chars={:?}",
                v.get("layout_boxes"),
                v.get("visible_boxes"),
                v.get("visible_text_chars")
            );
        }

        // B:新开小页(前台抢占验证)。
        let t1 = std::time::Instant::now();
        let out2 = mgr
            .new_page(
                "about:blank",
                BrowserMode::Headed,
                None,
                Some(1920),
                Some(1080),
                false,
                false,
                10_000,
                &browser::NavHints::default(),
            )
            .await
            .expect("open blank failed");
        println!(
            "[probe] B blank open done {}ms page_id={}",
            t1.elapsed().as_millis(),
            out2.page_id
        );
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        list_viewers("B");
        screencap(&format!("{outdir}/screen_B_blank.png"));

        // C:对彩色页 eval_js(交互补泵路径:重排+推帧),看查看器是否回彩页。
        let js = "(() => 'probe-eval-' + document.title)()";
        let top = json!({"action":"control","control_action":"eval_js","page_id":pid,
                         "params":{"js": js}});
        match api::control(&mgr, Some(&pid), "eval_js", &json!({"js": js}), &top).await {
            Ok(v) => println!("[probe] C eval: {}", clip(&v)),
            Err(e) => println!("[probe] C eval error: {} {}", e.code, e.message),
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        list_viewers("C");
        screencap(&format!("{outdir}/screen_C_after_eval.png"));

        mgr.shutdown().await;
        println!("[probe] done");
    });
}
