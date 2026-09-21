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
use crate::agent::window::{current_driver, probe_capability, Rect};
use crate::error::{AgentError, Result};

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

/// 从进程名推断目标应用名(2026-09-18 第 87 轮:补中文进程名映射)。
///
/// window_id 形如 `44978:0`(pid:wid)时无法直接推断,需先 list 拿 process_name
/// 再过本映射;微信 macOS 进程名可能是「微信」或「WeChat」。
pub(super) fn infer_app_name_from_process(process_name: &str) -> Option<String> {
    let lower = process_name.to_lowercase();
    if lower.contains("wechat") || lower.contains("weixin") || process_name.contains("微信") {
        return Some("WeChat".to_string());
    }
    if lower.contains("doubao") || process_name.contains("豆包") {
        return Some("Doubao".to_string());
    }
    if lower.contains("dingtalk") || process_name.contains("钉钉") {
        return Some("DingTalk".to_string());
    }
    if lower.contains("lark") || lower.contains("feishu") || process_name.contains("飞书") {
        return Some("Lark".to_string());
    }
    if lower.contains("qq") || process_name.contains("QQ") {
        return Some("QQ".to_string());
    }
    if lower.contains("meeting") || process_name.contains("会议") {
        return Some("腾讯会议".to_string());
    }
    None
}

/// window_id(pid:wid 形态)→ list_windows 查 process_name → 推断应用名。
fn infer_app_name_from_window_list(window_id: &str) -> Option<String> {
    let driver = current_driver();
    let wins = driver.list_windows(None).ok()?;
    let info = wins.into_iter().find(|w| w.id == window_id)?;
    infer_app_name_from_process(&info.process_name)
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
        "AX 已授权但屏录未授权:inspect/control 主路线完整可用;ocr/screenshot 不可用\
         (CGWindow 按窗口截取同样走 TCC 屏录门控,禁止重试);\
         聊天发送首选 chat_send —— macOS 无 click_point/input_field_path 时自动走\
         osascript_fallback 路线(activate + 前台守卫 + 输入框定位点击 + keystroke)。"
    } else if ax_available {
        "AX 已授权 + 屏录未授权 + 无坐标输入:直接走 MCP_Window_Use(action=chat_send, ..., chat_log_path=...) 用 osascript_fallback 路线发送,或 action=chat_loop 一次跑 N 轮。\
         screen_recording 授权路径:系统设置 → 隐私与安全性 → 屏幕录制 → 勾选宿主终端 → 完全退出并重启终端。"
    } else {
        "深度权限全无:仅 list/find 可用;读/操作需先到 系统设置 → 隐私与安全性 → 辅助功能 勾选宿主终端。"
    };
    let body = json!({
        "ok": true,
        "platform": std::env::consts::OS,
        "probe_method": if cfg!(target_os = "macos") {
            "CGPreflightScreenCaptureAccess(TCC 官方 API,第 87 轮起;语义与按窗口 OCR 截图一致)"
        } else {
            "platform_default"
        },
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
        // 第 88 轮语义修正:osascript_fallback 路线可用性(System Events keystroke 只需
        // AX 授权,与 coordinate_input 无关;旧写法 ax && !coord 在 AX✅+coord✅ 时报
        // false,但 chat_send 实际仍选 osascript_fallback,自相矛盾)。
        "osascript_fallback_available": ax_available,
        "recommended_route": recommended_route,
        "next_action": next_action,
        "hard_constraint": if cap.ocr_screenshot_cgwindow {
            "无"
        } else {
            "screen_recording=false:禁止调用 ocr/screenshot(必败),直接走 inspect 或 chat_send(osascript_fallback)"
        },
    });
    Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
}

// ===================== osascript_run(第 86 轮新增) =====================

// ===================== osascript 执行底座(2026-09-18 第 88 轮重写) =====================
//
// 历史教训(第 86/87 轮实现,llaew_20260918_151628.log 实证):
// 旧实现把脚本包进 shell 单引号 `osascript -e '<script>'` 且预先
// `.replace('"', "\\\"")` —— 单引号内 `\"` 原样保留,AppleScript 收到
// `tell application \"WeChat\" …` 直接语法错误 -2741,三调三败;
// 且 BashTool 对非零退出码仍返回 Ok(`<exit_code>1</exit_code>` 埋在 stdout),
// ok:true 假成功把 LLM 推向 BashTool 绕行(违反「窗口操控全程 MCP_Window_Use」约束)。
//
// 第 88 轮根治:不经 shell,`Command::new("osascript").arg("-e").arg(script)`
// argv 直传(脚本里可自由写双引号/反斜杠/多行 tell 块);真实 exit_code +
// 独立 stderr;try_wait 轮询 + 超时 kill。

/// osascript 一次执行的结构化结果。
/// 第 90 轮:去掉 macOS cfg 门控(非 macOS 存根同样引用本类型,否则 Windows 编译损坏)。
#[derive(Debug, Clone)]
pub(super) struct OsascriptOutcome {
    /// 退出码为 0。
    pub ok: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// 是否因超时被杀。
    pub timed_out: bool,
}

/// 直接执行 AppleScript 片段(argv 直传,不经 shell;macOS only)。
///
/// 多行脚本整体作为一个 `-e` 参数传入(osascript 会把所有 -e 拼接编译,
/// 单个含换行的 -e 合法)。超时(默认 5s)后 kill 子进程并返回 timed_out。
#[cfg(target_os = "macos")]
pub(super) fn osascript_exec(script: &str, timeout_ms: u64) -> Result<OsascriptOutcome> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| tool_err(MCP_WINDOW_USE_TOOL_NAME, format!("spawn osascript 失败: {e}")))?;
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms.max(500));
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                // 进程已退出,wait_with_output 立即返回并收干管道
                let out = child.wait_with_output().map_err(|e| {
                    tool_err(MCP_WINDOW_USE_TOOL_NAME, format!("回收 osascript 输出失败: {e}"))
                })?;
                let code = out.status.code().unwrap_or(-1);
                return Ok(OsascriptOutcome {
                    ok: out.status.success(),
                    exit_code: code,
                    stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
                    stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                    timed_out: false,
                });
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(OsascriptOutcome {
                        ok: false,
                        exit_code: -1,
                        stdout: String::new(),
                        stderr: format!("osascript 执行超时({}ms),已 kill", timeout_ms),
                        timed_out: true,
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(tool_err(
                    MCP_WINDOW_USE_TOOL_NAME,
                    format!("等待 osascript 退出失败: {e}"),
                ));
            }
        }
    }
}

/// 非 macOS 平台的 `osascript_exec` 存根(2026-09-19 第 90 轮补:
/// 第 88 轮把实现加了 `#[cfg(target_os = "macos")]` 但三个调用点
/// (run_osascript_fallback_send)未门控,Windows 编译损坏)。
///
/// Windows / Linux 上 osascript_fallback 路线在选择层(`run_chat_send` 路线
/// 分派)就不会被选中,本存根仅为编译完整性;被误调时返回结构化错误。
#[cfg(not(target_os = "macos"))]
pub(super) fn osascript_exec(_script: &str, _timeout_ms: u64) -> Result<OsascriptOutcome> {
    Err(tool_err(
        MCP_WINDOW_USE_TOOL_NAME,
        "osascript 仅 macOS 可用;Windows 请改用 control(send_keys/type_text)或 PowerShell",
    ))
}

/// `action=osascript_run` —— 直接执行 AppleScript 片段,绕开 BashTool 白名单。
///
/// macOS only;Windows 上返回结构化错误。
/// 参数:
/// - `osascript_script` *(必填)*:AppleScript 字符串(第 88 轮起 argv 直传 osascript,
///   不经 shell;脚本内双引号/反斜杠/多行 tell 块原样生效)
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
        let _ = (script, timeout_ms);
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            "osascript_run 仅 macOS 可用;Windows/Linux 请改用 control / PowerShell / BashTool",
        ));
    }

    #[cfg(target_os = "macos")]
    {
        // osascript_exec 自带 try_wait 轮询 + 超时 kill,直接调用
        // (本模块 async 上下文已有 std::thread::sleep 先例;最长阻塞 = timeout_ms)。
        let outcome = osascript_exec(&script, timeout_ms)?;
        let body = if outcome.ok {
            json!({
                "ok": true,
                "exit_code": outcome.exit_code,
                "stdout": outcome.stdout,
                "stderr": outcome.stderr,
                "engine": "osascript -e <argv 直传,不经 shell>(第 88 轮)",
                "next_action": "AppleScript 已执行成功;如需进一步操作,继续 osascript_run 或 chat_send"
            })
        } else {
            json!({
                "ok": false,
                "exit_code": outcome.exit_code,
                "timed_out": outcome.timed_out,
                "stdout": outcome.stdout,
                "stderr": outcome.stderr,
                "engine": "osascript -e <argv 直传,不经 shell>(第 88 轮)",
                "next_action": "AppleScript 执行失败(见 stderr 原文)。先按 stderr 修正脚本后用 osascript_run 重试;\
                                禁止改用 BashTool 执行 osascript 绕行(窗口操控必须全程 MCP_Window_Use);\
                                若提示辅助功能未授权(-1719/-1743),请到 系统设置 → 隐私与安全性 → 辅助功能 勾选宿主终端"
            })
        };
        return Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()));
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
    // 选择路线(第 86 轮新增 osascript_fallback;第 87 轮修正 macOS 无坐标/路径时的优先级):
    // - input_field_path + inspect_control  -> AX 控件树路线
    // - click_point + coordinate_input      -> 视觉路线(CGEvent 物理点击+键入)
    // - macOS + AX 已授权(无 click_point/input_field_path) -> osascript_fallback
    //   (activate + System Events keystroke 直达焦点控件;visual_no_input 裸 CGEvent
    //   打窗口根,实测微信输入框拿不到字符 —— 第 87 轮修正)
    // - 非 macOS + coordinate_input(无 click_point) -> visual_no_input(假定焦点已对)
    // - AX 已授权但屏录未授权 / 无 CGEvent  -> osascript_fallback(第 86 轮新增)
    // - 完全无授权                          -> degraded_no_op
    let ax_available = cfg!(target_os = "macos")
        && crate::agent::window::check_platform_permissions().accessibility;
    let route = if input_field_path.is_some() && cap.inspect_control {
        "ax"
    } else if click_point.is_some() && cap.coordinate_input {
        "visual"
    } else if ax_available {
        "osascript_fallback"
    } else if cap.coordinate_input {
        "visual_no_input"
    } else {
        "degraded_no_op"
    };
    // 2026-09-19 第 91 轮 P0-4:Windows visual_no_input 自动 upgrade
    #[cfg(target_os = "windows")]
    let effective_click_point: Option<(i64, i64)> = if route == "visual_no_input" {
        let wid_for_click = window_id.clone();
        let res = tokio::task::spawn_blocking(move || -> Result<(i64, i64)> {
            let info = lookup_window_info(&wid_for_click)?;
            // 第 109 轮:按进程名选布局覆盖(豆包=底部居中输入区)
            Ok(estimate_input_point_for(&info.process_name, &info.bounds))
        }).await;
        match res {
            Ok(Ok(p)) => Some(p),
            _ => None,
        }
    } else {
        None
    };
    #[cfg(not(target_os = "windows"))]
    let effective_click_point: Option<(i64, i64)> = None;
    // 确定 route:visual_no_input + effective_click_point 有值 升级 visual
    let mut route_owned: String = if route == "visual_no_input" && effective_click_point.is_some() {
        "visual".to_string()
    } else {
        route.to_string()
    };
    let route: &str = &route_owned;
    // effective_click_point 进 run_driver_send 时使用,run_chat_send 将 effective_click_point 传给 run_driver_send


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

    // 第 88 轮:前台焦点守卫 —— 所有非降级路线执行前先把目标窗口带到前台并轮询
    // 确认;拿不到前台立即结构化报错,**不盲打**(System Events keystroke / CGEvent
    // 都投递到当前焦点应用,旧实现无校验,用户切窗后消息会打进别的软件)。
    let mut frontmost_state: Option<FrontmostState> = None;
    if route != "degraded_no_op" {
        let guard = ensure_frontmost(&window_id).await?;
        if !guard.frontmost {
            append_chat_log(
                &chat_log_path,
                &format_chat_log_line(
                    now_unix(),
                    "FAIL",
                    &format!(
                        "window_id={window_id} text=\"{}\" route={route} stage=focus_acquire error=窗口 {}ms 内未前台",
                        truncate_for_log(&text, 80),
                        guard.elapsed_ms
                    ),
                ),
            );
            let body = json!({
                "ok": false,
                "window_id": window_id,
                "route": route_for_body,
                "capability": cap_tag_for_body,
                "stage": "focus_acquire",
                "frontmost_acquired": false,
                "chat_log_path": chat_log_path,
                "error": format!(
                    "窗口前置后 {}ms 内仍未前台(轮询 {} 次);用户可能正在操作其他窗口,\
                     已放弃本次发送防止误输入到其他软件",
                    guard.elapsed_ms, guard.attempts
                ),
                "next_action": "前台守卫未通过,本次未发送;可稍后重试 chat_send,或 action=open 重新激活目标窗口后再发"
            });
            return Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()));
        }
        frontmost_state = Some(guard);
    }

    // 执行阶段(osascript_fallback 走 osascript_exec argv 直传;其余走 driver)
    let exec_result: Result<String> = if route == "osascript_fallback" {
        run_osascript_fallback_send(&window_id, &text, &submit_key, &chat_log_path).await
    } else {
        run_driver_send(
            route,
            &window_id,
            effective_click_point,
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
        "frontmost_acquired": frontmost_state.map(|g| g.frontmost),
        "focus_acquire_ms": frontmost_state.map(|g| g.elapsed_ms),
        "chat_log_path": chat_log_path,
        "next_action": if verified {
            format!("已发送并 OCR 验证:{} 字符已在右侧对话显示框出现", text.chars().count())
        } else {
            "已发送(verified=false:可能 OCR 未识别 / 文本过短 / 走的是 osascript_fallback 路线无需 OCR;\
             前台守卫已通过,消息投递到目标窗口);可继续下一条或 chat_loop".to_string()
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
                    crate::agent::window::ControlAction::ClickPoint {
                        x: cx,
                        y: cy,
                        modifiers: None,
                    },
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

// ===================== 前台焦点守卫 + 输入框定位(2026-09-18 第 88 轮新增) =====================
//
// 用户实测痛点(llaew_20260918_151628.log + 现场反馈):
// - 「用户切换软件到其他窗口,输入的信息还是一样的」——System Events keystroke /
//   CGEvent 键入都投递到**当前焦点应用**,旧实现只在发送开头 activate 一次,
//   30s 间隔内用户切窗 → 后续消息全部打进别的应用;
// - 「看不到聊天窗口」——窗口未真正前置/未恢复最小化,且焦点未知时盲打。
//
// 对策:所有 chat_send 路线执行前 ensure_frontmost(bring_to_front + 轮询确认),
// 拿不到前台就结构化报错不盲打;fallback 路线在 keystroke 前再用窗口 bounds
// 比例估算点击输入框聚焦。

/// ensure_frontmost 轮询确认窗口前台的超时(毫秒)。
const FRONTMOST_TIMEOUT_MS: u64 = 1500;
/// ensure_frontmost 轮询间隔(毫秒)。
const FRONTMOST_POLL_MS: u64 = 150;

/// 前台确认结果(供应答体与日志观测)。
#[derive(Debug, Clone, Copy)]
pub(super) struct FrontmostState {
    /// 最终是否前台。
    pub frontmost: bool,
    /// 轮询次数。
    pub attempts: u32,
    pub elapsed_ms: u64,
}

/// 把窗口带到前台并轮询确认(跨平台走 WindowDriver trait)。
///
/// macOS = AXRaise + System Events frontmost;Windows = SetForegroundWindow 系;
/// fallback 平台 is_frontmost 默认 false、bring_to_front 默认 no-op ——
/// 为避免 Linux 误阻断,平台不支持真实前台探测时超时放行(frontmost=true 返回)。
/// (本模块 async 上下文已有 std::thread::sleep 先例;最长阻塞 = FRONTMOST_TIMEOUT_MS)
pub(super) async fn ensure_frontmost(window_id: &str) -> Result<FrontmostState> {
    let driver = current_driver();
    let started = std::time::Instant::now();
    let mut attempts = 0u32;
    // 先幂等前置一次(已前台时 bring_to_front 内部直接返回)
    let _ = driver.bring_to_front(window_id);
    loop {
        attempts += 1;
        if driver.is_frontmost(window_id) {
            return Ok(FrontmostState {
                frontmost: true,
                attempts,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }
        if started.elapsed().as_millis() as u64 >= FRONTMOST_TIMEOUT_MS {
            // 平台不支持前台探测(fallback 默认恒 false)时放行,避免误阻断;
            // macOS / Windows 实现了真实探测,超时即判定失败。
            let supported = cfg!(any(target_os = "macos", windows));
            return Ok(FrontmostState {
                frontmost: !supported,
                attempts,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }
        tokio::time::sleep(Duration::from_millis(FRONTMOST_POLL_MS)).await;
    }
}

/// 主流 IM(微信/钉钉/飞书/QQ)主界面输入框的比例估算坐标:
/// 右侧会话区下部 —— x = 左 + 72% 宽,y = 上 + 88% 高(微信 4.x 实测布局吻合)。
/// 返回屏幕绝对坐标。
pub(super) fn estimate_input_point(bounds: &Rect) -> (i64, i64) {
    (
        bounds.x + (bounds.width as f64 * 0.72) as i64,
        bounds.y + (bounds.height as f64 * 0.88) as i64,
    )
}

/// 第 109 轮:按进程名选输入框布局覆盖,再估算坐标。
///
/// 豆包等 Electron 聊天 UI 的输入框是**底部居中大输入区**(非 IM 右栏形态),
/// 默认 (0.72, 0.88) 会点到输入区右缘甚至消息区;豆包用 (0.50, 0.90)。
pub(super) fn estimate_input_point_for(process_name: &str, bounds: &Rect) -> (i64, i64) {
    let lower = process_name.to_lowercase();
    let (fx, fy) = if lower.contains("doubao") || process_name.contains("豆包") {
        (0.50, 0.90)
    } else {
        (0.72, 0.88)
    };
    (
        bounds.x + (bounds.width as f64 * fx) as i64,
        bounds.y + (bounds.height as f64 * fy) as i64,
    )
}

/// window_id → 最新 WindowInfo(list_windows 现查,保证 bounds 新鲜)。
fn lookup_window_info(window_id: &str) -> Result<crate::agent::window::WindowInfo> {
    let driver = current_driver();
    driver
        .list_windows(None)?
        .into_iter()
        .find(|w| w.id == window_id)
        .ok_or_else(|| {
            tool_err(
                MCP_WINDOW_USE_TOOL_NAME,
                format!(
                    "window_id={window_id} 不在当前可见窗口列表(可能已关闭/最小化/切到托盘);\
                     请先 action=list 或 action=open 重新定位"
                ),
            )
        })
}

/// 第 86 轮新增 / 第 88 轮重写:走 osascript System Events 的发送实现。
///
/// 第 88 轮改动(根治实测三连败 + 焦点漂移):
/// 1. 全部经 [`osascript_exec`] argv 直传(不经 shell,无引号腐蚀;真实 exit_code);
/// 2. 发送前 ensure_frontmost 前台守卫(拿不到前台 → 结构化报错,不盲打);
/// 3. keystroke 前按窗口 bounds 比例估算**点击输入框聚焦**(coordinate_input 可用时),
///    消除「会话切换后焦点不在输入框」的盲打;
/// 4. keystroke 后二次校验 frontmost,丢失则落盘 [FOCUS_LOST] + 重激活重试一次。
///
/// 第 109 轮改动(根治「消息打进别的软件」残余风险):
/// 5. 激活改 **pid 制** —— 旧实现按应用名 activate,映射表查不到时兜底
///    "WeChat"(豆包实测必错,激活的是微信)。现改为从 window_id 解析 pid,用
///    System Events `set frontmost of (first application process whose unix id is
///    {pid})` 定位(与 bring_to_front 同源,不会认错应用);app_name 仅作日志,
///    查不到记 "unknown",不再猜测。
/// 6. 输入框聚焦点击按进程名选布局覆盖(豆包底部居中输入区,见 estimate_input_point_for)。
///
/// 步骤:pid 前置(System Events)→ ensure_frontmost → click 输入框(CGEvent)
/// → keystroke(Unicode)→ frontmost 复检 → key code 36 (Return)。
async fn run_osascript_fallback_send(
    window_id: &str,
    text: &str,
    submit_key: &str,
    chat_log_path: &str,
) -> Result<String> {
    // 第 109 轮:app_name 仅作日志;激活不再按名(见函数头注释第 5 点)。
    let app_name = infer_target_app_name(window_id)
        .or_else(|| infer_app_name_from_window_list(window_id))
        .unwrap_or_else(|| "unknown".to_string());
    let ts0 = now_unix();

    let fail = |stage: &str, err: &str| -> AgentError {
        append_chat_log(
            chat_log_path,
            &format_chat_log_line(
                now_unix(),
                "FAIL",
                &format!(
                    "window_id={window_id} text=\"{}\" route=osascript_fallback stage={stage} error={}",
                    truncate_for_log(text, 80),
                    err
                ),
            ),
        );
        tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            format!("osascript_fallback {stage} 失败: {err}"),
        )
    };

    // 1. 前置到前台(pid 制;AX 已授权前提下的 System Events 调用,不会认错应用)。
    //    window_id 形如 "{pid}:{idx}";pid 解析失败时跳过本步,交由 ensure_frontmost
    //    的 bring_to_front(内部同样是 pid 制)兜底。
    if let Some(pid) = window_id.split_once(':').and_then(|(p, _)| p.parse::<i32>().ok()) {
        let frontmost = osascript_exec(
            &format!(
                "tell application \"System Events\" to set frontmost of (first application process whose unix id is {pid}) to true"
            ),
            5000,
        )?;
        if !frontmost.ok {
            return Err(fail("activate", &frontmost.stderr));
        }
    }
    std::thread::sleep(Duration::from_millis(200));

    // 2. 前台守卫:拿不到前台就不盲打(用户可能正操作其他窗口)
    let guard = ensure_frontmost(window_id).await?;
    if !guard.frontmost {
        return Err(fail(
            "focus_acquire",
            &format!(
                "activate 后 {}ms 内窗口仍未前台(轮询 {} 次);用户可能正在操作其他窗口,\
                 已放弃本次发送防止误输入;请稍后重试或提醒用户不要切换窗口",
                guard.elapsed_ms, guard.attempts
            ),
        ));
    }

    // 3. 点击输入框聚焦(coordinate_input 可用时;失败降级 warn 不阻断)。
    //    第 109 轮:按进程名选布局覆盖(豆包=底部居中输入区,其余=IM 右栏形态)。
    let cap = probe_capability();
    if cap.coordinate_input {
        let wid = window_id.to_string();
        let click_res = run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
            let info = lookup_window_info(&wid)?;
            let (px, py) = estimate_input_point_for(&info.process_name, &info.bounds);
            current_driver().act(
                &wid,
                "/",
                crate::agent::window::ControlAction::ClickPoint {
                    x: px,
                    y: py,
                    modifiers: None,
                },
            )?;
            Ok(format!("{px},{py}"))
        })
        .await;
        match click_res {
            Ok(pt) => {
                tracing::debug!(point = %pt, "osascript_fallback 输入框聚焦点击完成");
            }
            Err(e) => {
                tracing::warn!(error = %e, "osascript_fallback 输入框聚焦点击失败(降级继续 keystroke)");
            }
        }
        std::thread::sleep(Duration::from_millis(CHAT_SEND_TYPE_DELAY_MS));
    }

    // 4. keystroke(Unicode 中文走 "as Unicode text"),失败真实上报
    let keystroke = osascript_exec(
        &format!(
            "tell application \"System Events\" to keystroke \"{}\" as Unicode text",
            escape_applescript_string(text)
        ),
        15000,
    )?;
    if !keystroke.ok {
        return Err(fail("keystroke", &keystroke.stderr));
    }
    std::thread::sleep(Duration::from_millis(CHAT_SEND_ENTER_DELAY_MS));

    // 5. Enter 前二次校验前台(键入期间用户切窗 → Enter 会把内容发到别的应用)
    if !current_driver().is_frontmost(window_id) {
        append_chat_log(
            chat_log_path,
            &format_chat_log_line(
                now_unix(),
                "FOCUS_LOST",
                &format!(
                    "window_id={window_id} text=\"{}\" 键入后前台丢失,重新激活重试一次",
                    truncate_for_log(text, 80)
                ),
            ),
        );
        let guard2 = ensure_frontmost(window_id).await?;
        if !guard2.frontmost {
            return Err(fail(
                "focus_recheck",
                "键入后窗口前台丢失且重新激活失败;文本可能滞留在目标输入框,\
                 已跳过 Enter 防止误发送;请重新 chat_send 本条消息",
            ));
        }
    }

    // 6. submit_key(默认 enter / Return)
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
    let enter = osascript_exec(enter_script, 5000)?;
    if !enter.ok {
        return Err(fail("enter", &enter.stderr));
    }

    let _ = ts0;
    Ok(format!("osascript_fallback sent (app={app_name}, frontmost 守卫通过)"))
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
    // 2026-09-19 第 91 轮 P0-4:chat_loop 透传 click_point / input_field_path 给 chat_send
    let click_point = parse_point(args.get("click_point"));
    let input_field_path = get_str(&args, "input_field_path").map(str::to_string);
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
    // 第 88 轮:连续前台守卫失败计数(用户长时间操作其他窗口时及时止损,
    // 避免 30 条消息全部打进别的软件)。
    let mut consecutive_focus_fail = 0usize;
    const MAX_CONSECUTIVE_FOCUS_FAIL: usize = 3;
    let mut focus_aborted = false;

    for (i, msg) in messages.iter().take(total).enumerate() {
        // 发送(内部 chat_send 会自己落盘 [SEND] / [FAIL];第 88 轮起含前台守卫)
        let mut send_args = json!({
            "action": "chat_send",
            "window_id": window_id,
            "text": msg,
            "verify": false, // chat_loop 自己统一做 reply 检测
            "chat_log_path": chat_log_path,
        });
        // 2026-09-19 第 91 轮 P0-4:透传 click_point / input_field_path 给 chat_send
        if let Some((cx, cy)) = click_point {
            send_args["click_point"] = json!({"x": cx, "y": cy});
        }
        if let Some(ref p) = input_field_path {
            send_args["input_field_path"] = json!(p);
        }
        let (sent_ok, focus_failed, send_error) =
            match super::McpWindowUseTool.execute(send_args).await {
                Ok(s) => {
                    let ok = s.contains("\"ok\": true");
                    let focus_fail = s.contains("\"stage\": \"focus_acquire\"")
                        || s.contains("focus_acquire_failed")
                        || s.contains("focus_recheck");
                    (ok, focus_fail, None)
                }
                Err(e) => {
                    let msg_err = e.to_string();
                    let focus_fail = msg_err.contains("focus");
                    (false, focus_fail, Some(msg_err))
                }
            };
        if sent_ok {
            total_sent += 1;
            consecutive_focus_fail = 0;
        } else if focus_failed {
            consecutive_focus_fail += 1;
            append_chat_log(
                &chat_log_path,
                &format_chat_log_line(
                    now_unix(),
                    "FOCUS_LOST",
                    &format!(
                        "window_id={window_id} round={i} 连续前台守卫失败 {consecutive_focus_fail}/{MAX_CONSECUTIVE_FOCUS_FAIL}"
                    ),
                ),
            );
        } else {
            consecutive_focus_fail = 0;
        }

        // 回复检测(可选;osascript_fallback 路线下 OCR 不可用,跳过 reply 检测)
        // 第 88 轮:前台守卫失败的轮次跳过 reply 检测(消息根本没发进目标窗口)。
        let cap = probe_capability();
        let mut reply_seen = false;
        let mut reply_sample = String::new();
        if reply_detect && sent_ok && cap.ocr_screenshot_cgwindow {
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
            "error": send_error,
            "focus_failed": focus_failed,
            "reply_seen": reply_seen,
            "reply_sample": reply_sample,
            "elapsed_secs": started.elapsed().as_secs()
        }));

        // 第 88 轮 + 第 102 轮(M5):连续前台守卫失败时,先通过 HumanAssistHub
        // 向 TUI 询问用户:继续重试 / 暂停等待 / 立即中止。TUI 内弹出选择菜单,
        // 人工答复经 oneshot 回填;非 TUI 模式 fail-fast 沿用旧行为(直接止损)。
        if consecutive_focus_fail >= MAX_CONSECUTIVE_FOCUS_FAIL {
            // 仅在「本轮是最后一次可能的尝试」时打断用户 —— 不每条都问,
            // 避免在 30s 间隔里把用户刷到烦。
            let last_choice = crate::agent::human_assist::HumanAssistHub::global()
                .request(
                    "manual_verify",
                    &format!(
                        "chat_loop 连续 {MAX_CONSECUTIVE_FOCUS_FAIL} 轮前台守卫失败 ——                          目标窗口可能不在前台(用户切到其他窗口)。                         已发 {total_sent} 条,落盘到 {chat_log_path}。请选择:",
                    ),
                    vec![
                        "继续重试一次".to_string(),
                        "暂停等待(回前台后自动恢复)".to_string(),
                        "立即中止 chat_loop".to_string(),
                    ],
                    "",
                    "",
                    30000, // 30s 超时,超时 = 「立即中止」(保守兜底)
                )
                .await;
            match last_choice {
                crate::agent::human_assist::HumanAssistOutcome::Answered(answer)
                    if answer.contains("继续") =>
                {
                    consecutive_focus_fail = 0;
                    append_chat_log(
                        &chat_log_path,
                        &format_chat_log_line(
                            now_unix(),
                            "USER_RESUME",
                            "用户选择继续重试一次,重置焦点守卫计数",
                        ),
                    );
                }
                crate::agent::human_assist::HumanAssistOutcome::Answered(answer)
                    if answer.contains("暂停") =>
                {
                    // 暂停 = 阻塞等待 60s 后再试一次焦点守卫
                    append_chat_log(
                        &chat_log_path,
                        &format_chat_log_line(
                            now_unix(),
                            "USER_PAUSE",
                            "用户选择暂停等待 60s",
                        ),
                    );
                    std::thread::sleep(Duration::from_secs(60));
                    consecutive_focus_fail = 0;
                }
                _ => {
                    // 用户选择中止 / 超时 / 取消 / Unavailable → 止损
                    focus_aborted = true;
                    break;
                }
            }
        }
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
        "focus_aborted": focus_aborted,
        "target_query": target_query,
        "log": log,
        "chat_log_path": chat_log_path,
        "next_action": if focus_aborted {
            format!(
                "chat_loop 连续 {MAX_CONSECUTIVE_FOCUS_FAIL} 轮前台守卫失败已止损中止(已发 {total_sent} 条);\
                 用户可能正在操作其他窗口,继续发送会误输入到别的软件;\
                 请提醒用户保持目标窗口前台后重新发起 chat_loop"
            )
        } else if total_replies > 0 {
            format!(
                "chat_loop 完成 {} 轮,对方回复 {} 条,回复率 {:.1}%;\
                 可基于 log 撰写结构化聊天报告",
                total_sent, total_replies, reply_rate * 100.0
            )
        } else {
            format!(
                "chat_loop 完成 {} 轮,但全程未检测到对方回复;\
                 可能 target_query 不匹配 / OCR 不可用(屏录未授权时属于正常,reply_detect 自动跳过);\
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