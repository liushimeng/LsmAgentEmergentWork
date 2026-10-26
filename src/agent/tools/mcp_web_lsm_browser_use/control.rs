//! `MCP_Web_LsmAgentBrowser_Use(action=control, control_action=...)` 兼容层。
//!
//! 参数归一化、page_id 校验、任务锚点、人工介入桥接、访问台账记账;
//! 具体动作由 `agent::browser::api` 在进程内执行。

use serde_json::{json, Value};

use super::{bool_arg, engine_error, envelope, str_arg, target_anchor_guard, u64_arg, McpWebLsmBrowserUseTool};
use crate::agent::browser;
use crate::error::Result;

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
    let reason = str_arg(params, "reason")
        .or_else(|| str_arg(args, "reason"))
        .unwrap_or("custom")
        .to_ascii_lowercase();
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
                "hint": "可重试 request_human,或如实告知用户改在交互模式重试;严禁伪造结果。",
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
