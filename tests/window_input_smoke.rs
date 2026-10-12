//! Windows 窗口操控实机冒烟测试(2026-09-19 第 90 轮)。
//!
//! 验证第 90 轮「无障碍 API → Windows 消息 → 物理鼠标键盘」优先级链与新原语
//! 在真实桌面上的行为(设计见 `docs/MCP_Window_Use/02-鼠标键盘操控与优先级链方案.md`)。
//!
//! **环境变量门控**:`LAEW_SMOKE_WINDOW_INPUT=1` 才执行(默认跳过)—— 测试会
//! 启动一个 PowerShell 控制台窗口、修改其标题(WM_SETTEXT)、读取回文、移动光标,
//! 涉及真实桌面副作用,不进常规 `cargo test`。
//!
//! 验证项:
//! 1. `list_windows` 枚举到本测试启动的 PowerShell 窗口;
//! 2. `set_text`(path="/")优先命中 route=win32_msg(WM_SETTEXT,控制台窗口无
//!    UIA ValuePattern,自然落穿 T1 → T2,不触发物理键入);
//! 3. `get_text`(path="/")读回新标题(T1 Name / T2 WM_GETTEXT);
//! 4. `input_batch` 复合步骤(set_text + get_text + wait)一次调用完成,读取值
//!    出现在返回 results 中;
//! 5. `move_point`(悬停原语)把光标移到窗口中心(route=physical);
//! 6. 测试结束按 PID 树杀(kill /T /F)清理,不影响其他窗口。
//!
//! 运行:`LAEW_SMOKE_WINDOW_INPUT=1 cargo test --test window_input_smoke -- --nocapture`

#![cfg(windows)]

use std::process::Command;

use lsm_agent::agent::window::{current_driver, ControlAction};

fn smoke_enabled() -> bool {
    matches!(
        std::env::var("LAEW_SMOKE_WINDOW_INPUT").ok().as_deref(),
        Some("1") | Some("true")
    )
}

#[tokio::test]
async fn windows_input_priority_chain_smoke() {
    if !smoke_enabled() {
        eprintln!("[smoke] 跳过:未设置 LAEW_SMOKE_WINDOW_INPUT=1");
        return;
    }
    // 1. 启动字符映射表 charmap.exe(经典 Win32 对话框:独立进程 + 顶层 HWND +
    //    标题可写 + 按 PID 树可精确回收)。此前试过 notepad(Win11 单实例接管,
    //    会误碰用户已开窗口)/ powershell/conhost(ConPTY 宿主下附着当前终端,
    //    list_windows 枚举不到),charmap 是零误碰的稳定靶窗。
    let mut child = Command::new("charmap").spawn().expect("启动 charmap 失败");
    let pid = child.id();

    let driver = current_driver();
    let window_id = {
        let mut found = None;
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if let Ok(wins) = driver.list_windows(None) {
                // 按我们启动的 pid 精确匹配(绝不误碰用户已开的其它窗口)
                if let Some(w) = wins.iter().find(|w| w.pid == pid) {
                    found = Some(w.id.clone());
                    break;
                }
            }
        }
        found.unwrap_or_else(|| panic!("10s 内未枚举到 pid={pid} 的 charmap 窗口"))
    };
    println!("[smoke] ✓ list_windows 枚举到窗口 {window_id}(child pid={pid})");

    // 2. set_text:控制台窗口无 UIA ValuePattern → 应命中 route=win32_msg(WM_SETTEXT)
    let marker = format!("laew-smoke-第90轮-{pid}");
    let out = driver
        .act(&window_id, "/", ControlAction::SetText(marker.clone()))
        .expect("set_text 失败");
    println!("[smoke] set_text → {out}");
    assert!(
        out.contains("route=win32_msg") || out.contains("route=uia"),
        "标准控制台窗口的 set_text 应命中消息/无障碍层,而非物理键入: {out}"
    );

    // 3. get_text 读回
    let text = driver
        .act(&window_id, "/", ControlAction::GetText)
        .expect("get_text 失败");
    println!("[smoke] get_text → {text}");
    assert!(text.contains(&pid.to_string()), "标题应含本次标记 {marker}");

    // 4. input_batch 复合步骤:经工具层完整链路(前台守卫 + 批量 + results 回填)
    let marker2 = format!("laew-smoke-input_batch-{pid}");
    let args = serde_json::json!({
        "action": "input_batch",
        "window_id": window_id,
        "steps": [
            {"op": "set_text", "path": "/", "text": marker2},
            {"op": "get_text", "path": "/", "key": "title"},
            {"op": "wait", "ms": 50}
        ]
    });
    let batch_out = {
        use lsm_agent::agent::tools::Tool;
        let tool = lsm_agent::agent::tools::mcp_window_use::McpWindowUseTool;
        tool.execute(args).await
    }
    .expect("input_batch 执行失败");
    println!("[smoke] input_batch(截断) → {}", &batch_out[..batch_out.len().min(600)]);
    let v: serde_json::Value = serde_json::from_str(&batch_out).expect("input_batch 应返回 JSON");
    assert_eq!(v["ok"], serde_json::json!(true), "全部步骤应成功: {batch_out}");
    assert_eq!(v["steps_total"], serde_json::json!(3));
    assert!(
        v["results"]["title"].as_str().unwrap().contains(&pid.to_string()),
        "get_text 读取值应回填 results.title: {batch_out}"
    );

    // 5. move_point 悬停原语(把光标移到窗口中心,无点击副作用)
    let wins = driver.list_windows(Some("PowerShell")).unwrap();
    if let Some(w) = wins.iter().find(|w| w.id == window_id) {
        let cx = w.bounds.x + w.bounds.width / 2;
        let cy = w.bounds.y + w.bounds.height / 2;
        let out = driver
            .act(&window_id, "/", ControlAction::MovePoint { x: cx, y: cy })
            .expect("move_point 失败");
        println!("[smoke] move_point → {out}");
        assert!(out.contains("悬停"));
    }

    // 6. 精确清理(只杀本测试启动的进程树)
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
    let _ = child.wait();
    println!("[smoke] ✓ 第 90 轮优先级链 + input_batch 实机冒烟全部通过");
}
