//! MCP_Window_Use 复合 action(2026-09-18 第 85 轮新增):
//!
//! - `chat_send` —— 把"点击输入框 + 等 80ms + Unicode 键入 + 等 120ms + Enter +
//!   等 250ms + (可选)OCR 验证发送结果"封装成一次调用,消除原 4~5 步工具调用
//!   之间的焦点竞态。
//! - `chat_loop` —— 长时多轮会话循环工具,在工具内部循环 `chat_send`,可选地
//!   每轮 OCR 检测右侧对话显示框是否出现对方回复,返回结构化日志。
//!   SubAgent 一次调用就能跑 N 轮聊天,不需要在自身 agentic loop 里反复协调。
//!
//! 两个 action 平台门控同 `mcp_window_use_available()`,仅在 macOS / Windows
//! 注册。Linux 上定义但不进 builtin_registry。
//!
//! 设计见 `tmpPlan/2026-09-18_01-MCP_Window_Use微信桌面自动化能力全面强化方案.md`。

use std::time::Duration;

use serde_json::{json, Value};

use super::{
    driver_preflight, get_str, require_str, run_blocking, tool_err, McpWindowUseTool, Tool,
    MCP_WINDOW_USE_TOOL_NAME,
};
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

    if text.is_empty() {
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            "text 不能为空",
        ));
    }

    driver_preflight(MCP_WINDOW_USE_TOOL_NAME).await?;

    let cap = probe_capability();
    // 选择路线:
    // - 有 input_field_path + inspect_control 可用 -> 控件树路线(AX click + AX set_text + Enter)
    // - 有 click_point + coordinate_input 可用 -> 视觉路线(物理点击 + cg_type_text + Enter)
    // - 两者都不可用 -> 退化:依赖 osascript(System Events keystroke)通过 Bash 兜底
    let route = if input_field_path.is_some() && cap.inspect_control {
        "ax"
    } else if click_point.is_some() && cap.coordinate_input {
        "visual"
    } else if cap.coordinate_input {
        "visual_no_input" // 假定焦点已在输入框(用户可能已点击过)
    } else {
        "degraded_no_op" // 完全无能为力,告知 LLM 用 Bash + osascript
    };

    let route_for_closure = route.to_string();
    let route_for_body = route.to_string();
    let cap_tag_for_closure = cap.tag();
    let cap_tag_for_body = cap.tag();
    let submit_key_clone = submit_key.clone();
    let window_id_clone = window_id.clone();
    let text_clone = text.clone();
    let click_point_clone = click_point;
    let input_field_path_clone = input_field_path.clone();

    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || -> Result<String> {
        let driver = current_driver();
        match route_for_closure.as_str() {
            "ax" => {
                // 控件树路线:click 输入框 -> set_text -> 等 120ms -> send_keys(Enter)
                let path = input_field_path_clone
                    .ok_or_else(|| tool_err(MCP_WINDOW_USE_TOOL_NAME, "input_field_path 缺失"))?;
                driver.act(
                    &window_id_clone,
                    &path,
                    crate::agent::window::ControlAction::Focus,
                )?;
                std::thread::sleep(Duration::from_millis(CHAT_SEND_TYPE_DELAY_MS));
                driver.act(
                    &window_id_clone,
                    &path,
                    crate::agent::window::ControlAction::SetText(text_clone.clone()),
                )?;
                std::thread::sleep(Duration::from_millis(CHAT_SEND_ENTER_DELAY_MS));
                let submit = parse_submit_key(&submit_key_clone);
                driver.act(&window_id_clone, &path, submit)?;
            }
            "visual" => {
                let (cx, cy) = click_point_clone
                    .ok_or_else(|| tool_err(MCP_WINDOW_USE_TOOL_NAME, "click_point 缺失"))?;
                driver.act(
                    &window_id_clone,
                    "/",
                    crate::agent::window::ControlAction::ClickPoint { x: cx, y: cy },
                )?;
                std::thread::sleep(Duration::from_millis(CHAT_SEND_TYPE_DELAY_MS));
                driver.act(
                    &window_id_clone,
                    "/",
                    crate::agent::window::ControlAction::TypeText(text_clone.clone()),
                )?;
                std::thread::sleep(Duration::from_millis(CHAT_SEND_ENTER_DELAY_MS));
                let submit = parse_submit_key(&submit_key_clone);
                driver.act(&window_id_clone, "/", submit)?;
            }
            "visual_no_input" => {
                driver.act(
                    &window_id_clone,
                    "/",
                    crate::agent::window::ControlAction::TypeText(text_clone.clone()),
                )?;
                std::thread::sleep(Duration::from_millis(CHAT_SEND_ENTER_DELAY_MS));
                let submit = parse_submit_key(&submit_key_clone);
                driver.act(&window_id_clone, "/", submit)?;
            }
            "degraded_no_op" => {
                return Err(tool_err(
                    MCP_WINDOW_USE_TOOL_NAME,
                    format!(
                        "chat_send 当前 capability 不支持({}):无 input_field_path / click_point,且 coordinate_input=false;\
                         请改用 Bash + osascript 'tell application \"System Events\" to keystroke \"{{text}}\" & return'\
                         (需要辅助功能授权);或先授权辅助功能后重试",
                        cap_tag_for_closure
                    ),
                ));
            }
            _ => unreachable!(),
        }
        Ok("sent".into())
    })
    .await?;

    // 验证阶段(异步):等 verify_timeout_ms 后 OCR 右侧对话显示框,断言文本已上屏
    let verified = if verify && text.chars().count() >= CHAT_SEND_VERIFY_MIN_LEN {
        std::thread::sleep(Duration::from_millis(verify_timeout_ms));
        match verify_sent(&window_id, &text).await {
            Ok(true) => true,
            Ok(false) => false,
            Err(_) => false, // 验证失败不阻断 chat_send,仅标记 verified=false
        }
    } else {
        false
    };

    let body = json!({
        "ok": true,
        "window_id": window_id,
        "text_chars": text.chars().count(),
        "route": route_for_body,
        "capability": cap_tag_for_body,
        "verified": verified,
        "next_action": if verified {
            format!("已发送并 OCR 验证:{} 字符已在右侧对话显示框出现", text.chars().count())
        } else {
            "已发送(verified=false:可能 OCR 未识别 / 文本过短 / 窗口焦点丢失);可继续下一条或 chat_loop".to_string()
        }
    });
    Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
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
    // 拿窗口右侧 60% 区域(对话显示框典型布局)
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
    // 简单 contains;短文本(<5 字符)允许部分匹配(<=30% 容差 = 子串匹配失败时取字符集重叠)
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

    let total = messages.len().min(max_rounds);
    let mut log = Vec::with_capacity(total);
    let mut total_sent = 0usize;
    let mut total_replies = 0usize;
    let mut last_reply_text = String::new();
    let started = std::time::Instant::now();

    for (i, msg) in messages.iter().take(total).enumerate() {
        // 发送
        let send_args = json!({
            "action": "chat_send",
            "window_id": window_id,
            "text": msg,
            "verify": false  // chat_loop 自己统一做 reply 检测,避免每条都 OCR
        });
        let sent_ok = match McpWindowUseTool.execute(send_args).await {
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

        // 回复检测(可选)
        let mut reply_seen = false;
        let mut reply_sample = String::new();
        if reply_detect {
            // 给应用 250ms 把消息渲染上屏
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
        "next_action": if total_replies > 0 {
            format!("chat_loop 完成 {} 轮,对方回复 {} 条,回复率 {:.1}%;\
                     可基于 log 撰写结构化聊天报告",
                total_sent, total_replies, reply_rate * 100.0)
        } else {
            "chat_loop 完成,但全程未检测到对方回复;可能窗口焦点丢失 / target_query 不匹配 / OCR 失败;\
             建议读取 log + 重新 inspect/ocr 确认".to_string()
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
///
/// 用 `last_known` 作对比基线:OCR 拿到的文字拼接后去掉 `last_known` 前缀,如果
/// 剩余 ≥3 个非空字符即视为新消息。简单粗暴,但对微信 / 钉钉 类纯文本聊天足够。
async fn detect_new_reply(window_id: &str, last_known: &str) -> Result<bool> {
    let joined = ocr_right_panel(window_id).await?;
    if joined.len() <= last_known.len() {
        return Ok(false);
    }
    let new_part = &joined[last_known.len()..];
    let trimmed = new_part.trim();
    Ok(trimmed.chars().count() >= 3)
}

/// 提取窗口右侧对话显示框的最后一屏文字(返回完整 OCR 拼接,供 `last_known` 增量用)。
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
