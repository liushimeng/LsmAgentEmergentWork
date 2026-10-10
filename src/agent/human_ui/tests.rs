//! human_ui 纯函数与弹窗子进程链路单测(不触达真实弹窗窗口/Hub)。
//! 设计见 `docs/MCP_Web_Use/03-人工介入弹窗原生UI方案.md` §10。

use super::*;
use crate::agent::human_assist::HumanAssistDisplay;

/// env 变量读写全局态互斥。
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn sample_display() -> HumanAssistDisplay {
    HumanAssistDisplay {
        id: 7,
        kind: "sms".into(),
        message: "页面要求短信验证码,请输入后继续".into(),
        options: vec!["我已完成人工操作,继续".into(), "取消任务".into()],
        url: "https://example.com/login".into(),
        page_id: "p_ab12cd34".into(),
        image_path: String::new(),
        timeout_ms: 120_000,
        prompt_visual_width: 0,
        created_at_ms: 1_759_991_525_000,
    }
}

#[test]
fn payload_json_roundtrip() {
    let p = build_payload(&sample_display());
    assert_eq!(p["id"], 7);
    assert_eq!(p["kind"], "sms");
    assert_eq!(p["kind_label"], "短信验证码");
    assert_eq!(p["page_id"], "p_ab12cd34");
    assert_eq!(p["timeout_ms"], 120_000);
    assert_eq!(p["started_at_ms"], 1_759_991_525_000i64);
    assert_eq!(p["options"][0], "我已完成人工操作,继续");
    // 时间轴三要素齐备:提出时间 / 超时截止(已等待与剩余由弹窗动态刷新)
    let started = p["started_at"].as_str().expect("started_at");
    let deadline = p["deadline_at"].as_str().expect("deadline_at");
    assert!(started.len() >= 19, "started_at 应为 YYYY-MM-DD HH:MM:SS: {started}");
    assert!(deadline.len() >= 19, "deadline_at 同形: {deadline}");
    assert_ne!(started, deadline, "截止时刻应晚于提出时刻");
}

#[test]
fn payload_image_path_roundtrip_and_empty_default() {
    // 第 132 轮:image_path 透传到弹窗 payload(空串 = 无图)
    let mut d = sample_display();
    d.image_path = "/tmp/laew_hitl_captcha_1.png".into();
    let p = build_payload(&d);
    assert_eq!(p["image_path"], "/tmp/laew_hitl_captcha_1.png");
    // 默认空串也必须是字符串字段(而非 null),弹窗端按无图处理
    let empty = build_payload(&sample_display());
    assert_eq!(empty["image_path"], "");
}

#[test]
fn payload_kind_label_covers_all_reasons() {
    for (kind, label) in [
        ("captcha", "图形/滑块验证码"),
        ("sms", "短信验证码"),
        ("qr_login", "扫码登录"),
        ("login", "账密登录"),
        ("real_name", "实名认证/人脸核身"),
        ("two_factor", "二次验证/2FA"),
        ("oauth", "第三方授权"),
        ("manual_verify", "人工核验"),
        ("custom", "人工介入"),
    ] {
        let mut d = sample_display();
        d.kind = kind.into();
        assert_eq!(build_payload(&d)["kind_label"], label, "kind={kind}");
    }
}

#[test]
fn parse_result_answer_cancel_timeout_error() {
    assert_eq!(
        parse_result("{\"status\":\"answer\",\"text\":\"482913\"}\n"),
        UiResult::Answered("482913".into())
    );
    assert_eq!(parse_result("{\"status\":\"cancel\"}"), UiResult::Cancelled);
    assert_eq!(parse_result("{\"status\":\"timeout\",\"text\":\"\"}"), UiResult::Timeout);
    assert_eq!(
        parse_result("{\"status\":\"error\",\"text\":\"弹窗坏了\"}"),
        UiResult::Error("弹窗坏了".into())
    );
    // 空应答视为错误(不静默变成「已完成」)
    assert_eq!(
        parse_result("{\"status\":\"answer\",\"text\":\"  \"}"),
        UiResult::Error("弹窗返回空应答".into())
    );
}

#[test]
fn parse_result_tolerates_noise_and_unknown() {
    // 前面有噪声行,取最后一个合法 JSON
    let noisy = "2026-10-08 12:00:00 notice\n{\"status\":\"answer\",\"text\":\"ok\"}\n";
    assert_eq!(parse_result(noisy), UiResult::Answered("ok".into()));
    // 完全无法解析 → Error(降级 TUI,不 panic)
    assert!(matches!(parse_result("garbage"), UiResult::Error(_)));
    assert!(matches!(parse_result(""), UiResult::Error(_)));
}

/// 第 145 轮:extend 是**中间行**(「⏱ +2 分钟」延长通知,进程不退出),收口
/// 解析必须跳过它取真正的结果行;只有 extend 行时不得误当结果。
#[test]
fn parse_result_skips_extend_intermediate_lines() {
    let out = "{\"status\":\"extend\",\"id\":7,\"ms\":120000}\n{\"status\":\"answer\",\"text\":\"z7z2\"}\n";
    assert_eq!(parse_result(out), UiResult::Answered("z7z2".into()));
    assert!(matches!(
        parse_result("{\"status\":\"extend\",\"id\":7,\"ms\":120000}"),
        UiResult::Error(_)
    ));
}

/// 第 145 轮:extend 中间行解析(与 `dialog_main::extend_line` 互为往返)。
#[test]
fn parse_extend_line_shapes() {
    assert_eq!(
        parse_extend_line("{\"status\":\"extend\",\"id\":42,\"ms\":120000}"),
        Some((42, 120_000))
    );
    assert_eq!(
        parse_extend_line(&dialog_main::extend_line(9, 60_000)),
        Some((9, 60_000))
    );
    // 非延长行 / 缺 id / ms=0 / 非 JSON → None(容错,不 panic)
    assert_eq!(parse_extend_line("{\"status\":\"answer\",\"text\":\"ok\"}"), None);
    assert_eq!(parse_extend_line("{\"status\":\"extend\",\"ms\":1000}"), None);
    assert_eq!(parse_extend_line("{\"status\":\"extend\",\"id\":1,\"ms\":0}"), None);
    assert_eq!(parse_extend_line("garbage"), None);
    assert_eq!(parse_extend_line(""), None);
}

/// 第 145 轮:子进程链路对 extend 中间行的容错 —— 假弹窗先打延长行(此时无 hub
/// 槽位对应,延长不生效也不 panic)再打结果行,`present` 仍正确解析为应答,
/// 证明 stdout 已切换为逐行流式读。
#[cfg(unix)]
#[test]
fn present_tolerates_extend_lines_from_dialog_process() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    set_test_backend(None);
    let dir = std::env::temp_dir().join(format!("laew_ui_ext_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("fake-dialog-extend.sh");
    std::fs::write(
        &exe,
        "#!/bin/sh\n# 假弹窗:先打 extend 中间行再打结果 JSON\necho '{\"status\":\"extend\",\"id\":7,\"ms\":120000}'\nsleep 0.2\necho '{\"status\":\"answer\",\"text\":\"654321\"}'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("LAEW_HITL_DIALOG_EXE", &exe);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let out = rt.block_on(present(&sample_display()));
    std::env::remove_var("LAEW_HITL_DIALOG_EXE");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out, UiResult::Answered("654321".into()));
}

#[test]
fn enabled_test_override_and_env_off() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // cfg(test) 下默认关
    set_test_override(None);
    assert!(!enabled(), "单测构建默认不弹真窗");
    // 测试钩子强制开/关
    set_test_override(Some(true));
    assert!(enabled());
    set_test_override(Some(false));
    assert!(!enabled());
    // 强制关优先于 env
    std::env::set_var("LAEW_HUMAN_UI", "on");
    assert!(!enabled());
    std::env::remove_var("LAEW_HUMAN_UI");
    set_test_override(None);
}

#[test]
fn test_backend_is_used_by_present() {
    // 全局钩子:串行
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    fn stub(_d: &HumanAssistDisplay) -> UiResult {
        UiResult::Answered("1. stub".into())
    }
    set_test_backend(Some(stub));
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let out = rt.block_on(present(&sample_display()));
    set_test_backend(None);
    assert_eq!(out, UiResult::Answered("1. stub".into()));
}

/// 真实子进程链路(第 137 轮:`LAEW_HITL_DIALOG_EXE` 指向打印结果 JSON 的假
/// 可执行文件,不起真弹窗):覆盖「exe 定位 + spawn + stdout JSON 解析」全链。
#[cfg(unix)]
#[test]
fn present_spawns_dialog_process_with_env_exe() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    set_test_backend(None);
    let dir = std::env::temp_dir().join(format!("laew_ui_proc_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("fake-dialog.sh");
    std::fs::write(
        &exe,
        "#!/bin/sh\n# 假弹窗:忽略参数,打印结果 JSON\necho '{\"status\":\"answer\",\"text\":\"654321\"}'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("LAEW_HITL_DIALOG_EXE", &exe);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let out = rt.block_on(present(&sample_display()));
    std::env::remove_var("LAEW_HITL_DIALOG_EXE");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out, UiResult::Answered("654321".into()));
}

/// `LAEW_HITL_DIALOG_EXE` 指向不存在的文件:严格报错(不静默回落,便于排障)。
#[test]
fn env_exe_missing_is_strict_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    set_test_backend(None);
    std::env::set_var("LAEW_HITL_DIALOG_EXE", "/nonexistent/laew-dialog");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let out = rt.block_on(present(&sample_display()));
    std::env::remove_var("LAEW_HITL_DIALOG_EXE");
    assert!(
        matches!(out, UiResult::Error(ref e) if e.contains("LAEW_HITL_DIALOG_EXE")),
        "unexpected: {out:?}"
    );
}

/// 真弹窗冒烟(需 macOS/Windows 桌面会话;人工/半自动验证):弹窗真实弹出 4 秒后由
/// 代码 respond 收口(弹窗子进程被 kill_on_drop 回收)。默认跳过,显式 `-- --ignored` 运行。
/// 注意:经 `cargo test` 运行时 `current_exe()` 是 test harness,需先
/// `LAEW_HITL_DIALOG_EXE=$PWD/laew ./rebuild_restart_app.sh` 后带 env 跑本用例。
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[tokio::test]
#[ignore = "需桌面会话,手动冒烟: LAEW_HITL_DIALOG_EXE=./laew cargo test --lib smoke_real_dialog -- --ignored"]
async fn smoke_real_dialog_popup() {
    use crate::agent::human_assist::{AssistVia, HumanAssistHub};
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    set_test_backend(None);
    set_test_override(Some(true));
    let hub = HumanAssistHub::global();
    let was = hub.is_attached();
    hub.detach();
    let task = tokio::spawn({
        let hub = hub.clone();
        async move {
            hub.request(
                "captcha",
                "真弹窗冒烟:请观察弹窗是否置顶显示时间轴与选项(4 秒后自动收口)",
                vec!["我已完成人工操作,继续".into(), "取消任务".into()],
                "https://example.com/login",
                "p_smoke",
                60_000,
                "",
            )
            .await
        }
    });
    // 等弹窗事件
    let launched = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if hub.poll().is_some() {
                let evs = hub.take_events();
                if evs
                    .iter()
                    .any(|e| matches!(e, crate::agent::human_assist::AssistEvent::GuiLaunched { .. }))
                {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(launched.is_ok(), "弹窗应在 5s 内弹出");
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let d = hub.poll().expect("弹窗期间槽位仍在");
    assert!(hub.respond(d.id, Some("smoke-ok".into()), AssistVia::Tui));
    let out = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("收口应即时返回")
        .unwrap();
    assert!(
        matches!(
            out,
            crate::agent::human_assist::HumanAssistOutcome::Answered { ref text, .. } if text == "smoke-ok"
        ),
        "unexpected: {out:?}"
    );
    set_test_override(None);
    if was {
        hub.attach();
    }
}
