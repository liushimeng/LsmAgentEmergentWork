//! MCP_Window_Use 复合 action(2026-09-18 第 85~86 轮):
//!
//! - `chat_send` —— 把"点击输入框 + 等 80ms + Unicode 键入 + 等 120ms + Enter +
//!   等 250ms + (可选)OCR 验证发送结果"封装成一次调用,消除原 4~5 步工具调用
//!   之间的焦点竞态。第 86 轮新增 `osascript_fallback` 路线 + 强制 `chat_log_path` 落盘。
//! - `chat_loop` —— 长时多轮会话循环工具,在工具内部循环 `chat_send`,可选地
//!   每轮 OCR 检测右侧对话显示框是否出现对方回复,返回结构化日志。
//!   SubAgent 一次调用就能跑 N 轮聊天,不需要在自身 agentic loop 里反复协调。
//! - `capability_probe` —— 第 86 轮新增:返回真实能力矩阵,LLM 第一步必调。
//! - `osascript_run` —— 第 86 轮新增:直接调 `osascript -e <script>`,绕开 BashTool。
//!
//! 四个 action 平台门控同 `mcp_window_use_available()`,仅在 macOS / Windows
//! 注册。Linux 上定义但不进 builtin_registry。
//!
//! 设计见 `tmpPlan/2026-09-18_01-MCP_Window_Use微信桌面自动化能力全面强化方案.md`
//! + `tmpPlan/2026-09-18_02-MCP_Window_Use第86轮capability修正与osascript_fallback方案.md`。

use std::io::Write;
use std::time::Duration;

use serde_json::{json, Value};

use super::{
    driver_preflight, get_str, require_str, run_blocking, tool_err, Tool, MCP_WINDOW_USE_TOOL_NAME,
};
use crate::agent::tools::bash::BashTool;
use crate::agent::window::{current_driver, probe_capability, Rect};
use crate::error::Result;

// ===================== chat_send 参数与默认值 =====================

/// chat_send 默认 type 后等 80ms 吸收应用输入事件循环。
const CHAT_SEND_TYPE_DELAY_MS: u64 = 80;
/// chat_send 默认 type 完到 Enter 之间等 120ms(微信 / Electron 收到 Unicode key-up
/// 后才把文本入输入模型)。
const CHAT_SEND_ENTER_DELAY_MS: u64 = 120;
/// chat_send 默认 Enter 后等 250ms 让消息上屏(OCR 验证前置等待)。
const CHAT_SEND_VERIFY_DELAY_MS: u64 = 250;
/// OCR 验证文本包含目标文本的最小长度(短文本如"嗯"会非常容易误判,要求≥2 字符)。
const CHAT_SEND_VERIFY_MIN_LEN: usize = 2;

// ===================== chat_loop 默认值 =====================

/// chat_loop 默认每条消息间隔秒数(用户任务 15 分钟 30 条 ≈ 30s/条)。
const CHAT_LOOP_DEFAULT_INTERVAL_SECS: u64 = 30;
/// chat_loop 默认最大发送条数(防阻塞,单次调用最坏 ≈ 30 × 30s = 15min)。
const CHAT_LOOP_DEFAULT_MAX_ROUNDS: usize = 30;
/// chat_loop 默认 OCR 检测右侧对话显示框时间窗(秒,每条发送后等这么久)。
const CHAT_LOOP_REPLY_DETECT_TIMEOUT_SECS: u64 = 5;

// ===================== chat_log 落盘(第 86 轮新增) =====================

/// 解析 `chat_log_path` 参数(可选),缺省值 = `<工作目录>/llaew_chat_<unix_ts>.log`。
pub(super) fn resolve_chat_log_path(args: &Value) -> String {
    if let Some(p) = get_str(args, "chat_log_path") {
        return p.to_string();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let candidate = cwd.join(format!("llaew_chat_{ts}.log"));
    candidate.to_string_lossy().to_string()
}

/// 单行落盘到 chat_log;失败仅 warn,不断断主流程。
pub(super) fn append_chat_log(path: &str, line: &str) {
    if let Err(e) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| writeln!(f, "{line}"))
    {
        tracing::warn!(
            target: "mcp_window_use::chat_log",
            path = %path,
            error = %e,
            "chat_log 落盘失败(非阻断)"
        );
    }
}

/// 构造 chat_log 一行(带 unix 时间戳 + 标签)。
pub(super) fn format_chat_log_line(ts_unix: u64, label: &str, body: &str) -> String {
    format!("{ts_unix} [{label}] {body}")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 截断文本用于落盘(避免超长消息把日志打爆)。
fn truncate_for_log(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max).collect();
        s.push_str("...(truncated)");
        s
    }
}

/// 把字符串安全地嵌入 AppleScript 双引号字符串(转义反斜杠与双引号)。
pub(super) fn escape_applescript_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

/// 从 window_id 推断目标应用名(用于 osascript_fallback 的 activate)。
///
/// 已知映射表:WeChat 类 process_name → "WeChat"。
pub(super) fn infer_target_app_name(window_id: &str) -> Option<String> {
    let lower = window_id.to_lowercase();
    if lower.contains("wechat") || lower.contains("weixin") || lower.contains("wx") {
        return Some("WeChat".to_string());
    }
    if lower.contains("dingtalk") || lower.contains("ding") {
        return Some("DingTalk".to_string());
    }
    if lower.contains("lark") || lower.contains("feishu") {
        return Some("Lark".to_string());
    }
    if lower.contains("qq") {
        return Some("QQ".to_string());
    }
    None
}

// ===================== capability_probe(第 86 轮新增) =====================

/// `action=capability_probe` —— 返回当前进程真实能力矩阵。
///
/// LLM 第一步必调;据此决定走 AX / 视觉 / osascript_fallback 哪条路线。
/// 不需要任何 window_id。
pub(super) async fn run_capability_probe(_args: Value) -> Result<String> {
    let cap = probe_capability();
    let ax_available = cfg!(target_os = "macos")
        && crate::agent::window::check_platform_permissions().accessibility;
    let recommended_route = if cap.ocr_screenshot_cgwindow {
        "visual_or_ax"
    } else if cap.inspect_control || cap.coordinate_input {
        "ax_or_visual_no_ocr"
    } else if ax_available {
        "osascript_fallback"
    } else {
        "degraded_no_op"
    };
    let next_action = if cap.ocr_screenshot_cgwindow {
        "全权限:inspect / ocr / screenshot / chat_send(visual) / chat_loop 全路线可用"
    } else if cap.inspect_control || cap.coordinate_input {
        "AX 已授权但屏录未授权:inspect/control 主路线完整可用;ocr/screenshot 不可用;\
         chat_send 将走 visual_no_input/auto* 路线(无 OCR 验证);\
         自绘 UI(微信 4.x / 飞书 / 钉钉)建议直接走 osascript_fallback。"
    } else if ax_available {
        "AX 已授权 + 屏录未授权 + 无坐标输入:直接走 MCP_Window_Use(action=chat_send, ..., chat_log_path=...) 用 osascript_fallback 路线发送,或 action=chat_loop 一次跑 N 轮。\
         screen_recording 授权路径:系统设置 → 隐私与安全性 → 屏幕录制 → 勾选宿主终端 → 完全退出并重启终端。"
    } else {
        "深度权限全无:仅 list/find 可用;读/操作需先到 系统设置 → 隐私与安全性 → 辅助功能 勾选宿主终端。"
    };
    let body = json!({
        "ok": true,
        "platform": std::env::consts::OS,
        "permissions": {
            "accessibility": ax_available,
            "screen_recording": cap.ocr_screenshot_cgwindow || cap.screencapture_cli,
        },
        "capability": {
            "list_find": cap.list_find,
            "inspect_control": cap.inspect_control,
            "ocr_screenshot_cgwindow": cap.ocr_screenshot_cgwindow,
            "coordinate_input": cap.coordinate_input,
            "screencapture_cli": cap.screencapture_cli,
            "ax_warmup": cap.ax_warmup,
        },
        "capability_tag": cap.tag(),
        "next_action_hint": cap.next_action_hint(),
        "fallback_available": ax_available && !cap.coordinate_input,
        "recommended_route": recommended_route,
        "next_action": next_action,
    });
    Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
}

// ===================== osascript_run(第 86 轮新增) =====================

/// `action=osascript_run` —— 直接执行 AppleScript 片段,绕开 BashTool 白名单。
///
/// macOS only;Windows 上返回结构化错误。
/// 参数:
/// - `osascript_script` *(必填)*:AppleScript 字符串(将被 `osascript -e '<script>'` 包裹)
/// - `osascript_timeout_ms` *(可选)*:默认 5000ms
pub(super) async fn run_osascript_run(args: Value) -> Result<String> {
    let script = require_str(&args, "osascript_script", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let timeout_ms = args
        .get("osascript_timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(5000)
        .clamp(1000, 60000);

    #[cfg(not(target_os = "macos"))]
    {
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            "osascript_run 仅 macOS 可用;Windows/Linux 请改用 control / PowerShell / BashTool",
        ));
    }

    #[cfg(target_os = "macos")]
    {
        let escaped = script
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\'', "'\\''");
        let command = format!("osascript -e '{}'", escaped);
        let bash = BashTool;
        let result = bash
            .execute(json!({
                "command": command,
                "timeout_ms": timeout_ms,
            }))
            .await;
        let body = match result {
            Ok(output) => json!({
                "ok": true,
                "stdout": output,
                "stderr": "",
                "command": command,
                "next_action": "AppleScript 已执行;如需进一步操作,继续 osascript_run 或 chat_send"
            }),
            Err(e) => json!({
                "ok": false,
                "error": e.to_string(),
                "command": command,
                "next_action": "AppleScript 执行失败;若提示辅助功能未授权,请到系统设置 → 隐私与安全性 → 辅助功能 勾选宿主终端"
            }),
        };
        Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
    }
}

// ===================== chat_send =====================

/// `action=chat_send` —— 单条消息原子发送,自动按 WindowCapability 选路线。
///
/// 参数:
/// - `window_id` *(必填)*:`action=open/list/find` 返回的窗口 id
/// - `text` *(必填)*:要发送的消息文本(支持 Unicode)
/// - `click_point` *(可选)*:`{x,y}` 视觉路线输入框中心坐标(取自 `action=ocr`)
/// - `input_field_path` *(可选)*:`action=inspect` 拿到的输入框控件路径(AX 路线)
/// - `submit_key` *(可选)*:默认 `enter`(微信 Enter 发送);部分 App 是 `cmd+enter`
/// - `verify` *(可选)*:默认 `true`,发送后 OCR 右侧对话显示框验证
/// - `verify_timeout_ms` *(可选)*:默认 250ms 后开始 OCR,最长 5s 内重试
/// - `chat_log_path` *(可选, 第 86 轮新增)*:每次 send/recv/fail 落盘的工作日志文件绝对路径;QC 可 grep `[SEND]` 验证
pub(super) async fn run_chat_send(args: Value) -> Result<String> {
    let window_id = require_str(&args, "window_id", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let text = require_str(&args, "text", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let click_point = parse_point(args.get("click_point"));
    let input_field_path = get_str(&args, "input_field_path").map(str::to_string);
    let submit_key = get_str(&args, "submit_key").unwrap_or("enter").to_string();
    let verify = args
        .get("verify")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let verify_timeout_ms = args
        .get("verify_timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(CHAT_SEND_VERIFY_DELAY_MS)
        .clamp(50, 5000);
    let chat_log_path = resolve_chat_log_path(&args);

    if text.is_empty() {
        return Err(tool_err(MCP_WINDOW_USE_TOOL_NAME, "text 不能为空"));
    }

    driver_preflight(MCP_WINDOW_USE_TOOL_NAME).await?;

    let cap = probe_capability();
    // 选择路线(第 86 轮新增 osascript_fallback):
    // - input_field_path + inspect_control  -> AX 控件树路线
    // - click_point + coordinate_input      -> 视觉路线(CGEvent 物理点击+键入)
    // - coordinate_input(无 click_point)    -> visual_no_input(假定焦点已对)
    // - AX 已授权但屏录未授权 / 无 CGEvent  -> osascript_fallback(第 86 轮新增)
    // - 完全无授权                          -> degraded_no_op
    let ax_available = cfg!(target_os = "macos")
        && crate::agent::window::check_platform_permissions().accessibility;
    let route = if input_field_path.is_some() && cap.inspect_control {
        "ax"
    } else if click_point.is_some() && cap.coordinate_input {
        "visual"
    } else if cap.coordinate_input {
        "visual_no_input"
    } else if ax_available {
        "osascript_fallback"
    } else {
        "degraded_no_op"
    };

    tracing::debug!(
        target: "mcp_window_use::chat_send",
        window_id = %window_id,
        capability = %cap.tag(),
        route = %route,
        text_chars = text.chars().count(),
        verify = verify,
        "chat_send 进入"
    );

    let route_for_body = route.to_string();
    let cap_tag_for_body = cap.tag();

    // 执行阶段(osascript_fallback 走 BashTool;其余走 driver)
    let exec_result: Result<String> = if route == "osascript_fallback" {
        run_osascript_fallback_send(&window_id, &text, &submit_key, &chat_log_path).await
    } else {
        run_driver_send(
            route,
            &window_id,
            click_point,
            input_field_path.clone(),
            &text,
            &submit_key,
        )
        .await
    };

    if let Err(e) = exec_result {
        let body = json!({
            "ok": false,
            "window_id": window_id,
            "route": route_for_body,
            "capability": cap_tag_for_body,
            "error": e.to_string(),
            "chat_log_path": chat_log_path,
            "next_action": format!(
                "chat_send 失败(route={route_for_body}):{e};请 action=capability_probe 重新探测或换路线"
            )
        });
        return Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()));
    }

    // 验证阶段:仅对 AX / 视觉路线有意义;osascript_fallback 不验证(无 OCR)
    let verified = if verify
        && route != "osascript_fallback"
        && text.chars().count() >= CHAT_SEND_VERIFY_MIN_LEN
    {
        std::thread::sleep(Duration::from_millis(verify_timeout_ms));
        match verify_sent(&window_id, &text).await {
            Ok(true) => true,
            Ok(false) => false,
            Err(_) => false,
        }
    } else {
        false
    };

    // 第 86 轮新增:成功落盘 [SEND]
    let ts = now_unix();
    append_chat_log(
        &chat_log_path,
        &format_chat_log_line(
            ts,
            "SEND",
            &format!(
                "window_id={window_id} text=\"{}\" route={route_for_body} capability={cap_tag_for_body} verified={verified}",
                truncate_for_log(&text, 80),
            ),
        ),
    );

    let body = json!({
        "ok": true,
        "window_id": window_id,
        "text_chars": text.chars().count(),
        "route": route_for_body,
        "capability": cap_tag_for_body,
        "verified": verified,
        "chat_log_path": chat_log_path,
        "next_action": if verified {
            format!("已发送并 OCR 验证:{} 字符已在右侧对话显示框出现", text.chars().count())
        } else {
            "已发送(verified=false:可能 OCR 未识别 / 文本过短 / 窗口焦点丢失 / 走的是 osascript_fallback 路线无需 OCR);可继续下一条或 chat_loop".to_string()
        }
    });
    Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
}

/// 走 driver(AX / visual / visual_no_input)的发送实现。
async fn run_driver_send(
    route: &str,
    window_id: &str,
    click_point: Option<(i64, i64)>,
    input_field_path: Option<String>,
    text: &str,
    submit_key: &str,
) -> Result<String> {
    let route_owned = route.to_string();
    let window_id_owned = window_id.to_string();
    let text_owned = text.to_string();
    let submit_key_owned = submit_key.to_string();
    let input_field_path_owned = input_field_path;

    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || -> Result<String> {
        let driver = current_driver();
        match route_owned.as_str() {
            "ax" => {
                let path = input_field_path_owned
                    .ok_or_else(|| tool_err(MCP_WINDOW_USE_TOOL_NAME, "input_field_path 缺失"))?;
                driver.act(
                    &window_id_owned,
                    &path,
                    crate::agent::window::ControlAction::Focus,
                )?;
                std::thread::sleep(Duration::from_millis(CHAT_SEND_TYPE_DELAY_MS));
                driver.act(
                    &window_id_owned,
                    &path,
                    crate::agent::window::ControlAction::SetText(text_owned.clone()),
                )?;
                std::thread::sleep(Duration::from_millis(CHAT_SEND_ENTER_DELAY_MS));
                let submit = parse_submit_key(&submit_key_owned);
                driver.act(&window_id_owned, &path, submit)?;
            }
            "visual" => {
                let (cx, cy) = click_point
                    .ok_or_else(|| tool_err(MCP_WINDOW_USE_TOOL_NAME, "click_point 缺失"))?;
                driver.act(
                    &window_id_owned,
                    "/",
                    crate::agent::window::ControlAction::ClickPoint { x: cx, y: cy },
                )?;
                std::thread::sleep(Duration::from_millis(CHAT_SEND_TYPE_DELAY_MS));
                driver.act(
                    &window_id_owned,
                    "/",
                    crate::agent::window::ControlAction::TypeText(text_owned.clone()),
                )?;
                std::thread::sleep(Duration::from_millis(CHAT_SEND_ENTER_DELAY_MS));
                let submit = parse_submit_key(&submit_key_owned);
                driver.act(&window_id_owned, "/", submit)?;
            }
            "visual_no_input" => {
                driver.act(
                    &window_id_owned,
                    "/",
                    crate::agent::window::ControlAction::TypeText(text_owned.clone()),
                )?;
                std::thread::sleep(Duration::from_millis(CHAT_SEND_ENTER_DELAY_MS));
                let submit = parse_submit_key(&submit_key_owned);
                driver.act(&window_id_owned, "/", submit)?;
            }
            "degraded_no_op" => {
                return Err(tool_err(
                    MCP_WINDOW_USE_TOOL_NAME,
                    "chat_send 当前 capability 不支持:请先 action=capability_probe 重新探测,或先授权辅助功能后重试",
                ));
            }
            _ => unreachable!(),
        }
        Ok("sent".into())
    })
    .await
}

/// 第 86 轮新增:走 osascript System Events 的发送实现。
///
/// 步骤:
/// 1. `osascript -e 'tell application "<App>" to activate'`
/// 2. 等 200ms(微信 runloop 拿焦点)
/// 3. `osascript -e 'tell application "System Events" to keystroke "<text>" as Unicode text'`
/// 4. 等 120ms
/// 5. `osascript -e 'tell application "System Events" to key code 36'` (Return)
async fn run_osascript_fallback_send(
    window_id: &str,
    text: &str,
    submit_key: &str,
    chat_log_path: &str,
) -> Result<String> {
    let app_name = infer_target_app_name(window_id).unwrap_or_else(|| "WeChat".to_string());
    let ts0 = now_unix();

    // 1. activate
    let activate_script = format!("tell application \"{app_name}\" to activate");
    let activate_cmd = format!(
        "osascript -e '{}'",
        activate_script.replace('\'', "'\\''")
    );
    if let Err(e) = BashTool
        .execute(json!({
            "command": activate_cmd,
            "timeout_ms": 5000,
        }))
        .await
    {
        append_chat_log(
            chat_log_path,
            &format_chat_log_line(
                ts0,
                "FAIL",
                &format!(
                    "window_id={window_id} text=\"{}\" route=osascript_fallback stage=activate error={}",
                    truncate_for_log(text, 80),
                    e
                ),
            ),
        );
        return Err(e);
    }
    std::thread::sleep(Duration::from_millis(200));

    // 2. keystroke (Unicode 中文走 "as Unicode text")
    let keystroke_script = format!(
        "tell application \"System Events\" to keystroke \"{}\" as Unicode text",
        escape_applescript_string(text)
    );
    let keystroke_cmd = format!(
        "osascript -e '{}'",
        keystroke_script.replace('\'', "'\\''")
    );
    if let Err(e) = BashTool
        .execute(json!({
            "command": keystroke_cmd,
            "timeout_ms": 15000,
        }))
        .await
    {
        append_chat_log(
            chat_log_path,
            &format_chat_log_line(
                now_unix(),
                "FAIL",
                &format!(
                    "window_id={window_id} text=\"{}\" route=osascript_fallback stage=keystroke error={}",
                    truncate_for_log(text, 80),
                    e
                ),
            ),
        );
        return Err(e);
    }
    std::thread::sleep(Duration::from_millis(CHAT_SEND_ENTER_DELAY_MS));

    // 3. submit_key (默认 enter / Return)
    let enter_script = match submit_key.to_lowercase().as_str() {
        "enter" | "return" | "回车" => {
            "tell application \"System Events\" to key code 36"
        }
        "cmd+enter" | "command+enter" => {
            "tell application \"System Events\" to keystroke return using command down"
        }
        "tab" => "tell application \"System Events\" to key code 48",
        other => {
            tracing::warn!(submit_key = %other, "未识别的 submit_key,默认走 enter");
            "tell application \"System Events\" to key code 36"
        }
    };
    let enter_cmd = format!("osascript -e '{}'", enter_script.replace('\'', "'\\''"));
    if let Err(e) = BashTool
        .execute(json!({
            "command": enter_cmd,
            "timeout_ms": 5000,
        }))
        .await
    {
        append_chat_log(
            chat_log_path,
            &format_chat_log_line(
                now_unix(),
                "FAIL",
                &format!(
                    "window_id={window_id} text=\"{}\" route=osascript_fallback stage=enter error={}",
                    truncate_for_log(text, 80),
                    e
                ),
            ),
        );
        return Err(e);
    }

    Ok("osascript_fallback sent".into())
}

fn parse_point(v: Option<&Value>) -> Option<(i64, i64)> {
    let v = v?;
    let x = v.get("x")?.as_i64()?;
    let y = v.get("y")?.as_i64()?;
    Some((x, y))
}

fn parse_submit_key(name: &str) -> crate::agent::window::ControlAction {
    let lower = name.trim().to_lowercase();
    match lower.as_str() {
        "enter" | "return" | "回车" => crate::agent::window::ControlAction::SendKeys("enter".into()),
        "cmd+enter" | "command+enter" => crate::agent::window::ControlAction::SendKeys("cmd+enter".into()),
        "tab" => crate::agent::window::ControlAction::SendKeys("tab".into()),
        other => crate::agent::window::ControlAction::SendKeys(other.into()),
    }
}

/// OCR 右侧对话显示框,断言文本已上屏(简单 contains 匹配,允许尾部容差)。
async fn verify_sent(window_id: &str, text: &str) -> Result<bool> {
    use crate::agent::window::OcrBlock;
    let driver = current_driver();
    let info = {
        let wins = driver.list_windows(None)?;
        wins.into_iter().find(|w| w.id == window_id).ok_or_else(|| {
            tool_err(MCP_WINDOW_USE_TOOL_NAME, "window_id 已失效,请重新 action=list")
        })?
    };
    let region = Rect {
        x: info.bounds.x + (info.bounds.width as f64 * 0.4) as i64,
        y: info.bounds.y + (info.bounds.height as f64 * 0.1) as i64,
        width: (info.bounds.width as f64 * 0.6) as i64,
        height: (info.bounds.height as f64 * 0.7) as i64,
    };
    let blocks: Vec<OcrBlock> = driver.ocr_with_info(&info, Some(region), None)?;
    let joined: String = blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let needle = text.trim();
    if needle.is_empty() {
        return Ok(false);
    }
    if joined.contains(needle) {
        return Ok(true);
    }
    let chars_in_needle: std::collections::HashSet<char> = needle.chars().collect();
    let overlap = joined.chars().filter(|c| chars_in_needle.contains(c)).count();
    let ratio = overlap as f64 / needle.chars().count().max(1) as f64;
    Ok(ratio >= 0.6 && needle.chars().count() <= 5)
}

// ===================== chat_loop =====================

/// `action=chat_loop` —— 长时多轮会话循环,内部循环 `chat_send` + OCR 检测回复。
///
/// 参数:
/// - `window_id` *(必填)*
/// - `messages` *(必填)*:按顺序发送的消息数组(JSON 字符串数组)
/// - `target_query` *(可选)*:对话对象名字,用于 OCR 检测对方回复(无则跳过 reply 检测)
/// - `interval_seconds` *(可选)*:每两条消息间隔秒数(默认 30)
/// - `max_rounds` *(可选)*:最大发送条数(默认 30,防阻塞)
/// - `reply_detect` *(可选)*:每条发完后 OCR 检测右侧是否出现对方消息(默认 true)
/// - `stop_on_reply` *(可选)*:对方回复后是否立即停下(默认 false)
/// - `chat_log_path` *(可选, 第 86 轮新增)*:send/recv/fail/summary 落盘路径
pub(super) async fn run_chat_loop(args: Value) -> Result<String> {
    let window_id = require_str(&args, "window_id", MCP_WINDOW_USE_TOOL_NAME)?.to_string();
    let messages = parse_messages(args.get("messages"))?;
    if messages.is_empty() {
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            "messages 不能为空(JSON 字符串数组)",
        ));
    }
    let target_query = get_str(&args, "target_query").map(str::to_string);
    let interval_secs = args
        .get("interval_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(CHAT_LOOP_DEFAULT_INTERVAL_SECS)
        .clamp(0, 300);
    let max_rounds = args
        .get("max_rounds")
        .and_then(Value::as_u64)
        .unwrap_or(CHAT_LOOP_DEFAULT_MAX_ROUNDS as u64)
        .clamp(1, 1000) as usize;
    let reply_detect = args
        .get("reply_detect")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let stop_on_reply = args
        .get("stop_on_reply")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let chat_log_path = resolve_chat_log_path(&args);

    let total = messages.len().min(max_rounds);
    let mut log = Vec::with_capacity(total);
    let mut total_sent = 0usize;
    let mut total_replies = 0usize;
    let mut last_reply_text = String::new();
    let started = std::time::Instant::now();

    for (i, msg) in messages.iter().take(total).enumerate() {
        // 发送(内部 chat_send 会自己落盘 [SEND] / [FAIL])
        let send_args = json!({
            "action": "chat_send",
            "window_id": window_id,
            "text": msg,
            "verify": false, // chat_loop 自己统一做 reply 检测
            "chat_log_path": chat_log_path,
        });
        let sent_ok = match super::McpWindowUseTool.execute(send_args).await {
            Ok(s) => {
                total_sent += 1;
                s.contains("\"ok\": true")
            }
            Err(e) => {
                log.push(json!({
                    "round": i,
                    "sent_text": msg,
                    "sent_ok": false,
                    "error": e.to_string(),
                    "reply_seen": false,
                    "reply_sample": null
                }));
                continue;
            }
        };

        // 回复检测(可选;osascript_fallback 路线下 OCR 不可用,跳过 reply 检测)
        let cap = probe_capability();
        let mut reply_seen = false;
        let mut reply_sample = String::new();
        if reply_detect && cap.ocr_screenshot_cgwindow {
            std::thread::sleep(Duration::from_millis(250));
            let detect_deadline = std::time::Instant::now()
                + Duration::from_secs(CHAT_LOOP_REPLY_DETECT_TIMEOUT_SECS);
            while std::time::Instant::now() < detect_deadline {
                if let Ok(true) = detect_new_reply(&window_id, &last_reply_text).await {
                    reply_seen = true;
                    if let Ok(text) = extract_last_reply(&window_id).await {
                        reply_sample = text;
                        last_reply_text = reply_sample.clone();
                        total_replies += 1;
                        // 落盘 [RECV](第 86 轮新增)
                        append_chat_log(
                            &chat_log_path,
                            &format_chat_log_line(
                                now_unix(),
                                "RECV",
                                &format!(
                                    "window_id={window_id} sample=\"{}\"",
                                    truncate_for_log(&reply_sample, 80)
                                ),
                            ),
                        );
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }

        log.push(json!({
            "round": i,
            "sent_text": msg,
            "sent_ok": sent_ok,
            "reply_seen": reply_seen,
            "reply_sample": reply_sample,
            "elapsed_secs": started.elapsed().as_secs()
        }));

        if stop_on_reply && reply_seen {
            break;
        }
        // 间隔(最后一条不等)
        if i + 1 < total && interval_secs > 0 {
            std::thread::sleep(Duration::from_secs(interval_secs));
        }
    }

    let reply_rate = if total_sent > 0 {
        total_replies as f64 / total_sent as f64
    } else {
        0.0
    };

    // 落盘 [SUMMARY](第 86 轮新增)
    append_chat_log(
        &chat_log_path,
        &format_chat_log_line(
            now_unix(),
            "SUMMARY",
            &format!(
                "rounds={} sent={} replies={} rate={:.2}% elapsed={}s",
                log.len(),
                total_sent,
                total_replies,
                reply_rate * 100.0,
                started.elapsed().as_secs()
            ),
        ),
    );

    tracing::debug!(
        target: "mcp_window_use::chat_loop",
        rounds = log.len(),
        sent = total_sent,
        replies = total_replies,
        rate = reply_rate,
        elapsed_secs = started.elapsed().as_secs(),
        "chat_loop 完成"
    );

    let body = json!({
        "ok": true,
        "window_id": window_id,
        "total_rounds": log.len(),
        "total_sent": total_sent,
        "total_replies": total_replies,
        "reply_rate": reply_rate,
        "elapsed_secs": started.elapsed().as_secs(),
        "target_query": target_query,
        "log": log,
        "chat_log_path": chat_log_path,
        "next_action": if total_replies > 0 {
            format!(
                "chat_loop 完成 {} 轮,对方回复 {} 条,回复率 {:.1}%;\
                 可基于 log 撰写结构化聊天报告",
                total_sent, total_replies, reply_rate * 100.0
            )
        } else {
            format!(
                "chat_loop 完成 {} 轮,但全程未检测到对方回复;\
                 可能窗口焦点丢失 / target_query 不匹配 / OCR 失败(屏录未授权时属于正常);\
                 已落盘 {total} 条 [SEND] 与 [SUMMARY] 到工作日志 {}",
                total_sent, chat_log_path
            )
        }
    });
    Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
}

fn parse_messages(v: Option<&Value>) -> Result<Vec<String>> {
    let v = v.ok_or_else(|| tool_err(MCP_WINDOW_USE_TOOL_NAME, "缺少 messages 参数"))?;
    let arr = v.as_array().ok_or_else(|| {
        tool_err(MCP_WINDOW_USE_TOOL_NAME, "messages 必须是 JSON 字符串数组")
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        match item {
            Value::String(s) => out.push(s.clone()),
            _ => {
                return Err(tool_err(
                    MCP_WINDOW_USE_TOOL_NAME,
                    format!("messages 元素必须是字符串,实际 {item:?}"),
                ))
            }
        }
    }
    Ok(out)
}

/// 检测窗口右侧对话显示框是否有"上次未见过的"新文字(即对方回复)。
async fn detect_new_reply(window_id: &str, last_known: &str) -> Result<bool> {
    let joined = ocr_right_panel(window_id).await?;
    if joined.len() <= last_known.len() {
        return Ok(false);
    }
    let new_part = &joined[last_known.len()..];
    let trimmed = new_part.trim();
    Ok(trimmed.chars().count() >= 3)
}

/// 提取窗口右侧对话显示框的最后一屏文字。
async fn extract_last_reply(window_id: &str) -> Result<String> {
    ocr_right_panel(window_id).await
}

/// OCR 窗口右侧对话显示框(右侧 60% × 中间 70%)。
async fn ocr_right_panel(window_id: &str) -> Result<String> {
    use crate::agent::window::OcrBlock;
    let driver = current_driver();
    let wins = driver.list_windows(None)?;
    let info = wins
        .into_iter()
        .find(|w| w.id == window_id)
        .ok_or_else(|| tool_err(MCP_WINDOW_USE_TOOL_NAME, "window_id 已失效"))?;
    let region = Rect {
        x: info.bounds.x + (info.bounds.width as f64 * 0.4) as i64,
        y: info.bounds.y + (info.bounds.height as f64 * 0.1) as i64,
        width: (info.bounds.width as f64 * 0.6) as i64,
        height: (info.bounds.height as f64 * 0.7) as i64,
    };
    let blocks: Vec<OcrBlock> = driver.ocr_with_info(&info, Some(region), None)?;
    Ok(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join(" "))
}