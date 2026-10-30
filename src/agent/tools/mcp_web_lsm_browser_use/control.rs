//! `MCP_Web_LsmAgentBrowser_Use(action=control, control_action=...)` 兼容层。
//!
//! 参数归一化、page_id 校验、任务锚点、人工介入桥接、访问台账记账;
//! 具体动作由 `agent::browser::api` 在进程内执行。

use serde_json::{json, Value};

use super::{bool_arg, engine_error, envelope, str_arg, target_anchor_guard, u64_arg, McpWebLsmBrowserUseTool};
use crate::agent::browser;
use crate::agent::web_control::{self, ControlState, Engine};
use crate::error::Result;

/// 人工介入原因白名单(与 `human_assist::HUMAN_REASONS` 同源,此处保留本地副本
/// 仅为报错时给出 allowed 清单)。
const HUMAN_REASONS: &[&str] = &[
    "captcha",
    "sms",
    "qr_login",
    "login",
    "real_name",
    "two_factor",
    "oauth",
    "manual_verify",
    "custom",
];

pub(super) async fn run(mut args: Value) -> Result<String> {
    let Some(action) = str_arg(&args, "control_action").map(str::to_string) else {
        return envelope(1001, "缺少 control_action", json!({}));
    };
    let params = normalize_params(&mut args);
    if action == "request_human" {
        return request_human(&args, &params).await;
    }
    let (page_id, page_auto) = match super::resolve_page_id(&args).await {
        Ok(v) => v,
        Err(msg) => {
            return envelope(1001, &msg, json!({"control_action": action}));
        }
    };
    let mgr = McpWebLsmBrowserUseTool::mgr();
    if !mgr.page_exists(&page_id).await {
        // 走 EngineError::page_not_found:跨工具 page_id 误用(Chrome 的 c_ 前缀)
        // 在此给出可操作指引 —— 2026-10-26 实测本路径此前直发通用文案,
        // 导致文档 41 §F4 的跨工具诊断成为死代码。
        return Ok(super::engine_error(browser::EngineError::page_not_found(&page_id)));
    }
    // 控制权状态切换:Agent 控制状态 ⇄ 用户控制状态(两态互斥)。
    if action == "set_control_state" {
        let raw = str_arg(&params, "state")
            .or_else(|| str_arg(&args, "state"))
            .unwrap_or("agent");
        let Some(state) = ControlState::parse(raw) else {
            return envelope(
                1001,
                "set_control_state 的 state 非法(取值 agent|user)",
                json!({"state": raw, "allowed": ["agent", "user"]}),
            );
        };
        let note = str_arg(&params, "note").or_else(|| str_arg(&args, "note"));
        web_control::set_state(Engine::Lsm, &page_id, state, note, "manual");
        // Lsm 侧蒙层在**查看器帧合成层**绘制(page_bars::draw),下一次推帧即生效;
        // 这里主动触发一帧刷新,让状态切换立刻可见。
        mgr.refresh_control_state(&page_id).await;
        return envelope(
            0,
            "ok",
            json!({
                "page_id": page_id,
                "page_id_auto": page_auto,
                "control_action": action,
                "control_state": state.as_str(),
                "control_state_label": state.label(),
                "result": {
                    "control_state": state.as_str(),
                    "mask_active": web_control::mask_enabled() && state == ControlState::Agent,
                    "hint": match state {
                        ControlState::Agent => "已取回控制权:蒙层在位,Agent 可正常操作页面",
                        ControlState::User => "已交还控制权:蒙层撤除;Agent 的页面写操作会被拒绝(4004),直到再次 set_control_state=agent",
                    },
                }
            }),
        );
    }
    // **控制权闸门**:用户控制状态下 Agent 的页面写操作一律拒绝(4004);
    // 观测类动作(inspect/screenshot/wait)始终放行。
    if let Err(denied) = web_control::guard_agent_write(Engine::Lsm, &page_id, &action) {
        return envelope(denied.code, &denied.message, denied.data);
    }
    // 显式导航动作继续受任务锚点约束。
    if matches!(action.as_str(), "navigate" | "new_tab" | "new_window") {
        if let Some(url) = str_arg(&params, "url") {
            if let Some(blocked) = target_anchor_guard(url) {
                return blocked;
            }
        }
    }
    let params = lift_top_level_values(&args, params, &action);
    let out = browser::api::control(&mgr, Some(&page_id), &action, &params, &args).await;
    match out {
        Ok(mut data) => {
            if !data.is_object() {
                data = json!({"result": data});
            }
            data["page_id"] = json!(page_id);
            data["page_id_auto"] = json!(page_auto);
            data["control_state"] = json!(web_control::state_of(Engine::Lsm, &page_id).as_str());
            record_visit(&page_id, &action, &data);
            envelope(0, "ok", data)
        }
        Err(e) => Ok(engine_error(e)),
    }
}

/// 把 `params` 统一成对象;兼容 LLM 把 params 写成 JSON 字符串的形态。
fn normalize_params(args: &mut Value) -> Value {
    let params = match args.get("params").cloned() {
        Some(Value::Object(o)) => Value::Object(o),
        Some(Value::String(s)) => serde_json::from_str::<Value>(&s)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({})),
        _ => json!({}),
    };
    // 顶层便捷别名并入 params(与旧契约一致)。
    let mut obj = params.as_object().cloned().unwrap_or_default();
    for key in [
        "message", "reason", "options", "url", "timeout_ms", "force", "selector", "nth",
        "text", "key", "keys", "file_paths", "file_path", "name", "value", "kind", "items",
        "save_path", "output_path", "path", "full_page", "ocr", "clip", "region",
        "expression", "script", "function", "js", "code", "await_promise",
        "return_base64", "format", "duration_ms", "ms", "state", "values", "index",
    ] {
        if let Some(v) = args.get(key) {
            obj.entry(key.to_string()).or_insert_with(|| v.clone());
        }
    }
    let merged = Value::Object(obj);
    args["params"] = merged.clone();
    merged
}

fn lift_top_level_values(args: &Value, mut params: Value, action: &str) -> Value {
    let _ = args;
    if action == "request_human" {
        return params;
    }
    if let Some(obj) = params.as_object_mut() {
        // 旧契约允许 control 的 url 写在顶层。
        if !obj.contains_key("url") {
            if let Some(url) = args.get("url") {
                obj.insert("url".to_string(), url.clone());
            }
        }
    }
    params
}

fn record_visit(page_id: &str, action: &str, data: &Value) {
    let final_url = data
        .get("final_url")
        .or_else(|| data.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if final_url.is_empty() {
        return;
    }
    let source = if matches!(action, "new_tab" | "new_window") {
        super::visit_ledger::VisitSource::NewTab
    } else if action == "navigate" {
        super::visit_ledger::VisitSource::Navigate
    } else {
        return;
    };
    let title = data.get("title").and_then(Value::as_str).unwrap_or("");
    super::visit_ledger::record_visit(page_id, final_url, title, source);
}

// ===================== 人工介入 =====================

async fn request_human(args: &Value, params: &Value) -> Result<String> {
    // 自由文本 reason 先按关键词归一(与 Chrome 侧共用 `human_assist::normalize_reason`),
    // 否则「京东要求登录后才能访问」这类描述会被判非法,白白浪费一轮往返。
    let raw_reason = str_arg(params, "reason")
        .or_else(|| str_arg(args, "reason"))
        .unwrap_or("custom");
    let reason = crate::agent::human_assist::normalize_reason(raw_reason);
    if !HUMAN_REASONS.contains(&reason.as_str()) {
        return envelope(
            1001,
            "request_human reason 非法",
            json!({"reason": reason, "allowed": HUMAN_REASONS}),
        );
    }
    let message = str_arg(params, "message")
        .or_else(|| str_arg(args, "message"))
        .map(str::to_string)
        .unwrap_or_else(|| crate::agent::human_assist::default_message(&reason).to_string());
    let options: Vec<String> = params
        .get("options")
        .or_else(|| args.get("options"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).take(8).map(str::to_string).collect())
        .unwrap_or_default();
    let multi_select = bool_arg(params, "multi_select", false);
    let default_timeout = match reason.as_str() {
        "captcha" | "sms" | "two_factor" => 120_000,
        _ => 300_000,
    };
    let timeout_ms = u64_arg(params, "timeout_ms")
        .or_else(|| u64_arg(args, "timeout_ms"))
        .unwrap_or(default_timeout);
    let page_id = str_arg(args, "page_id").unwrap_or("");
    let mgr = McpWebLsmBrowserUseTool::mgr();
    let url = if page_id.is_empty() {
        String::new()
    } else {
        mgr.page_summary(page_id)
            .await
            .map(|p| p.url)
            .unwrap_or_default()
    };
    let image_path = str_arg(params, "image_path").unwrap_or("");

    // 人工可见性守卫:隐藏模式(或查看器已退出)时人工没有可见窗口可操作。
    // 如实把系统提示拼进展示消息、并在结果里回 page_visible:false,避免
    // 「请在浏览器窗口完成滑块」这类不可能指令发给人工(实测 -f 后台管道
    // 任务默认 hidden,人工侧只看得到 HITL 弹窗)。
    // 第一步先尝试复活:查看器曾被拉起过(页为有头)但中途退出(人工误关/
    // 崩溃)时,直接重建窗口比让人工对着「隐藏模式」提示干等强——弹窗本
    // 身就在等人工,人工却没有窗口可操作,任务必然卡死。
    let mut page_visible = mgr.viewer_alive();
    if !page_visible && !page_id.is_empty() {
        page_visible = mgr.revive_viewer().await;
    }
    let message = if page_visible {
        message
    } else {
        format!(
            "{message}\n[系统提示] 当前浏览器为隐藏模式(无可见窗口),人工无法直接操作页面;\
             可在弹窗选项中应答,或让 Agent 以有头模式(headed)重新打开页面后再发起人工介入。"
        )
    };

    // **控制权移交**:提问期间控制权交还人工 —— 撤蒙层;收口后按进入前的状态恢复。
    let hitl_note = format!("人工介入中({reason});完成后由 Agent 取回控制权");
    let prev_state = if page_id.is_empty() {
        ControlState::Agent
    } else {
        web_control::enter_user_control(Engine::Lsm, page_id, Some(hitl_note.as_str()), "hitl")
    };
    if !page_id.is_empty() {
        mgr.refresh_control_state(page_id).await;
    }

    let hub = crate::agent::human_assist::HumanAssistHub::global();
    let outcome = hub
        .request(
            &reason,
            &message,
            options,
            &url,
            page_id,
            timeout_ms,
            image_path,
            true,
            multi_select,
            None,
        )
        .await;
    // 收口:恢复进入前的控制权状态(应答/超时/取消三条路径都走到这里)。
    if !page_id.is_empty() {
        web_control::restore_control(Engine::Lsm, page_id, prev_state, "hitl_end");
        mgr.refresh_control_state(page_id).await;
    }
    use crate::agent::human_assist::HumanAssistOutcome;
    match outcome {
        HumanAssistOutcome::Answered { text, via } => {
            let next_hint = if text.chars().all(|c| c.is_ascii_digit() || c.is_whitespace()) {
                "看起来是验证码/数字码:用 control(input_text) 填入对应输入框并提交"
            } else {
                "人工已完成或给出了补充说明:继续 inspect 验证现场后再决定下一步"
            };
            envelope(
                0,
                "ok",
                json!({
                    "reason": reason,
                    "kind_label": crate::agent::human_assist::kind_label(&reason),
                    "human_response": text,
                    "assist_channel": via.as_str(),
                    "origin": "llm",
                    "page_visible": page_visible,
                    "next_hint": next_hint,
                }),
            )
        }
        HumanAssistOutcome::Timeout { via } => envelope(
            4001,
            "人工介入等待超时",
            json!({
                "reason": reason,
                "kind_label": crate::agent::human_assist::kind_label(&reason),
                "assist_channel": via.as_str(),
                "page_visible": page_visible,
                "hint": if page_visible {
                    "可重试 request_human,或如实告知用户改在交互模式重试;严禁伪造结果。"
                } else {
                    "页面无可见窗口(隐藏模式),重试 request_human 前先以有头模式打开页面,否则人工永远无法在页面上操作;严禁伪造结果。"
                },
            }),
        ),
        HumanAssistOutcome::Cancelled { via } => {
            crate::agent::human_cancel::set(&reason);
            envelope(
                4002,
                "人工取消任务",
                json!({
                    "reason": reason,
                    "human_cancelled": true,
                    "assist_channel": via.as_str(),
                    "page_visible": page_visible,
                    "next_hint": "立即停止本任务,不要重试或改用其它通道。",
                }),
            )
        }
        HumanAssistOutcome::Unavailable => envelope(
            4001,
            "人工介入通道不可用",
            json!({
                "reason": reason,
                "kind_label": crate::agent::human_assist::kind_label(&reason),
                "hint": "桌面弹窗与 TUI 均不可用;请如实告知用户改在交互模式(-p 之外)重试。",
            }),
        ),
    }
}
