//! human_ui 纯函数与脚本查找序单测(不触达真实弹窗进程/Hub)。
//! 设计见 docs/MCP_Web_Use/03-人工介入弹窗UI动态加载方案.md §9。

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
    // 时间轴三要素齐备:提出时间 / 超时截止(已等待与剩余由脚本动态刷新)
    let started = p["started_at"].as_str().expect("started_at");
    let deadline = p["deadline_at"].as_str().expect("deadline_at");
    assert!(started.len() >= 19, "started_at 应为 YYYY-MM-DD HH:MM:SS: {started}");
    assert!(deadline.len() >= 19, "deadline_at 同形: {deadline}");
    assert_ne!(started, deadline, "截止时刻应晚于提出时刻");
}

#[test]
fn payload_image_path_roundtrip_and_empty_default() {
    // 第 132 轮:image_path 透传到弹窗 payload(空串 = 无图,旧脚本忽略该字段)
    let mut d = sample_display();
    d.image_path = "/tmp/laew_hitl_captcha_1.png".into();
    let p = build_payload(&d);
    assert_eq!(p["image_path"], "/tmp/laew_hitl_captcha_1.png");
    // 默认空串也必须是字符串字段(而非 null),JXA/PowerShell 端按 falsy 处理
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
        parse_result("{\"status\":\"error\",\"text\":\"脚本坏了\"}"),
        UiResult::Error("脚本坏了".into())
    );
    // 空应答视为错误(不静默变成「已完成」)
    assert_eq!(
        parse_result("{\"status\":\"answer\",\"text\":\"  \"}"),
        UiResult::Error("弹窗返回空应答".into())
    );
}

#[test]
fn parse_result_tolerates_noise_and_unknown() {
    // 前面有 osascript 噪声行,取最后一个合法 JSON
    let noisy = "2026-10-08 12:00:00 notice\n{\"status\":\"answer\",\"text\":\"ok\"}\n";
    assert_eq!(parse_result(noisy), UiResult::Answered("ok".into()));
    // 完全无法解析 → Error(降级 TUI,不 panic)
    assert!(matches!(parse_result("garbage"), UiResult::Error(_)));
    assert!(matches!(parse_result(""), UiResult::Error(_)));
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
fn script_env_override_missing_file_is_strict_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var("LAEW_HUMAN_UI_SCRIPT", "/nonexistent/laew_ui.js");
    let r = resolve_script();
    std::env::remove_var("LAEW_HUMAN_UI_SCRIPT");
    assert!(r.is_err(), "显式脚本缺失应报错(不静默回落内置)");
}

#[test]
fn script_env_override_takes_precedence() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = std::env::temp_dir().join(format!("laew_ui_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("custom.js");
    std::fs::write(&file, "// custom\nfunction run(a){ return '{}'; }").unwrap();
    std::env::set_var("LAEW_HUMAN_UI_SCRIPT", &file);
    let r = resolve_script().expect("应加载显式脚本");
    std::env::remove_var("LAEW_HUMAN_UI_SCRIPT");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(r.origin, ScriptOrigin::Path(file));
    assert!(r.body.contains("custom"));
}

#[test]
fn script_falls_back_to_embedded_when_no_override() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("LAEW_HUMAN_UI_SCRIPT");
    let r = resolve_script().expect("内置兜底必成功");
    // 本机可能存在用户覆盖文件,故只断言「有正文 + 来源合法」
    assert!(!r.body.is_empty(), "脚本正文不应为空");
    match &r.origin {
        ScriptOrigin::Embedded => {
            // 内置脚本必须包含结果契约关键字
            // 第 132 轮:macOS 弹窗已由 NSAlert+runModal 重写为自绘 NSWindow
            //(可选中复制 / first responder / 定时器 / 验证码图片),断言同步更新。
            #[cfg(target_os = "macos")]
            assert!(r.body.contains("NSWindow"), "内置 JXA 应含自绘 NSWindow");
            #[cfg(target_os = "windows")]
            assert!(r.body.contains("Windows.Forms"), "内置 PS 应含 WinForms");
        }
        ScriptOrigin::Path(p) => assert!(p.is_file()),
    }
}

#[test]
fn materialize_embedded_writes_and_cleans_temp() {
    let resolved = ResolvedScript {
        origin: ScriptOrigin::Embedded,
        body: "// test".into(),
    };
    let path;
    {
        let m = resolved.materialize().expect("落盘成功");
        assert!(m.path.is_file());
        path = m.path.clone();
        assert!(m.owned);
    }
    assert!(!path.exists(), "Drop 后临时脚本应清理");
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

/// 真实子进程链路(stub 脚本经 LAEW_HUMAN_UI_SCRIPT 动态加载,不起真弹窗):
/// 覆盖「脚本查找序 env 优先 + 进程 spawn + stdout JSON 解析」全链。
#[cfg(target_os = "macos")]
#[test]
fn present_runs_env_script_stub_process() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    set_test_backend(None);
    let dir = std::env::temp_dir().join(format!("laew_ui_proc_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("stub.js");
    std::fs::write(&script, "function run(a){ return '{\"status\":\"answer\",\"text\":\"654321\"}'; }")
        .unwrap();
    std::env::set_var("LAEW_HUMAN_UI_SCRIPT", &script);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let out = rt.block_on(present(&sample_display()));
    std::env::remove_var("LAEW_HUMAN_UI_SCRIPT");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out, UiResult::Answered("654321".into()));
}

/// 真弹窗冒烟(需 macOS 桌面会话;人工/半自动验证):弹窗真实弹出 4 秒后由
/// 代码 respond 收口(弹窗进程被回收)。默认跳过,显式 `-- --ignored` 运行。
#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "需桌面会话,手动冒烟: cargo test --lib smoke_real_dialog -- --ignored"]
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
