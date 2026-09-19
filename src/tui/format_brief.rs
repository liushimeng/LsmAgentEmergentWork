//! TUI 工具输出摘要子模块(2026-09-18 第 88 轮新增)。
//!
//! 拆分背景:`format.rs` 已达 1700+ 临界线,按「单源码文件 ≤1800 行」规范,
//! 新增的 MCP_Window_Use 输出摘要函数落到本职责子模块;
//! 既有 `browser_output_brief` / `tool_args_brief` 仍在 format.rs(未机械搬移,
//! 后续轮次如需继续增长可整体迁入)。

use super::format::truncate_chars;

/// MCP_Window_Use 成功输出的关键字段摘要(第 88 轮;第 90 轮 +input_batch)。
///
/// 原始 output_summary 是 pretty JSON,截 80 字只看得到 `"ok": true` 前缀噪声。
/// 这里按 action 产物提取排查最关心的字段(解析失败回退 None → 调用方截断):
/// - chat_send:`route / verified / frontmost_acquired`;
/// - chat_loop:`rounds / sent / replies / focus_aborted`;
/// - input_batch(第 90 轮):`steps_total / steps_ok`;
/// - run_sequence(第 91 轮):`steps_total / steps_ok` + `focus_lost_count / retried_steps / focus_aborted`;
/// - osascript_run:`exit_code / stderr 首行`;
/// - ocr:`block_count`;open/find:`window_id`;任意失败:`error`。
pub(crate) fn window_use_output_brief(output_summary: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(output_summary.trim()).ok()?;
    if !v.is_object() {
        return None;
    }
    let get_s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    let get_b = |k: &str| v.get(k).and_then(|x| x.as_bool());
    let get_n = |k: &str| v.get(k).and_then(|x| x.as_i64());
    let mut parts: Vec<String> = Vec::new();
    if let Some(route) = Some(get_s("route")).filter(|s| !s.is_empty()) {
        parts.push(format!("route={route}"));
        if let Some(vf) = get_b("verified") {
            parts.push(format!("verified={vf}"));
        }
        if let Some(fm) = get_b("frontmost_acquired") {
            parts.push(format!("frontmost={}", if fm { "✓" } else { "✗" }));
        }
    }
    // 第 90 轮:input_batch 批处理摘要(总步数 / 成功步数)。
    if let Some(total) = get_n("steps_total") {
        let ok = get_n("steps_ok").unwrap_or(0);
        parts.push(format!("steps={ok}/{total}"));
    }
    // 第 91 轮:run_sequence 连续工作模式摘要(焦点丢失 / 重试 / 止损)。
    if let Some(n) = get_n("focus_lost_count") {
        if n > 0 {
            parts.push(format!("focus_lost={n}"));
        }
    }
    if let Some(n) = get_n("retried_steps") {
        if n > 0 {
            parts.push(format!("retries={n}"));
        }
    }
    if get_b("focus_aborted") == Some(true) && get_n("total_sent").is_none() {
        parts.push("focus_aborted!".to_string());
    }
    if let Some(sent) = get_n("total_sent") {
        parts.push(format!(
            "rounds={} sent={sent} replies={}",
            get_n("total_rounds").unwrap_or(0),
            get_n("total_replies").unwrap_or(0)
        ));
        if get_b("focus_aborted") == Some(true) {
            parts.push("focus_aborted!".to_string());
        }
    }
    if let Some(code) = get_n("exit_code") {
        parts.push(format!("exit={code}"));
        if code != 0 {
            let err_first = get_s("stderr").lines().next().unwrap_or("");
            if !err_first.is_empty() {
                parts.push(format!("err=\"{}\"", truncate_chars(err_first, 36)));
            }
        }
    }
    if let Some(n) = get_n("block_count") {
        parts.push(format!("blocks={n}"));
    }
    let wid = get_s("window_id");
    if !wid.is_empty() && parts.is_empty() {
        parts.push(format!("wid={wid}"));
    }
    if let Some(err) = Some(get_s("error")).filter(|s| !s.is_empty()) {
        parts.push(format!("err=\"{}\"", truncate_chars(err, 40)));
    }
    if parts.is_empty() {
        return None;
    }
    Some(truncate_chars(&parts.join(" "), 80))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_chat_send_route_fields() {
        let out = r#"{
            "ok": true,
            "window_id": "44978:0",
            "route": "osascript_fallback",
            "verified": false,
            "frontmost_acquired": true,
            "chat_log_path": "/tmp/x.log"
        }"#;
        let b = window_use_output_brief(out).expect("应能解析");
        assert!(b.contains("route=osascript_fallback"), "{b}");
        assert!(b.contains("verified=false"), "{b}");
        assert!(b.contains("frontmost=✓"), "{b}");
    }

    #[test]
    fn brief_chat_loop_rounds_and_focus_abort() {
        let out = r#"{"ok": true, "total_rounds": 5, "total_sent": 4, "total_replies": 1, "focus_aborted": true}"#;
        let b = window_use_output_brief(out).unwrap();
        assert!(b.contains("rounds=5 sent=4 replies=1"), "{b}");
        assert!(b.contains("focus_aborted!"), "{b}");
    }

    #[test]
    fn brief_osascript_run_exit_code_with_stderr() {
        let out = r#"{"ok": false, "exit_code": 1, "stderr": "17:18: syntax error (-2741)\nmore"}"#;
        let b = window_use_output_brief(out).unwrap();
        assert!(b.contains("exit=1"), "{b}");
        assert!(b.contains("syntax error"), "{b}");
        // 成功时不带 err
        let ok = window_use_output_brief(r#"{"ok": true, "exit_code": 0}"#).unwrap();
        assert_eq!(ok, "exit=0");
    }

    #[test]
    fn brief_fallback_for_unparseable_or_empty() {
        assert!(window_use_output_brief("not json").is_none());
        assert!(window_use_output_brief("[]").is_none());
        // 纯 ok:true 无任何关键字段 → None(调用方回退截断)
        assert!(window_use_output_brief(r#"{"ok": true}"#).is_none());
        // ocr blocks
        let b = window_use_output_brief(r#"{"block_count": 42}"#).unwrap();
        assert_eq!(b, "blocks=42");
    }

    #[test]
    fn brief_input_batch_round90() {
        // 第 90 轮:input_batch 摘要 steps=成功/总数。
        let out = r#"{"ok": true, "action": "input_batch", "steps_total": 5, "steps_ok": 5}"#;
        let b = window_use_output_brief(out).unwrap();
        assert!(b.contains("steps=5/5"), "{b}");
        // 部分失败
        let out = r#"{"ok": false, "action": "input_batch", "steps_total": 4, "steps_ok": 2}"#;
        let b = window_use_output_brief(out).unwrap();
        assert!(b.contains("steps=2/4"), "{b}");
    }

    #[test]
    fn brief_run_sequence_round91() {
        // 第 91 轮:run_sequence 摘要 steps + focus_lost + retries。
        let out = r#"{"ok": true, "action": "run_sequence", "steps_total": 12, "steps_ok": 12, "focus_lost_count": 1, "retried_steps": 2, "focus_aborted": false}"#;
        let b = window_use_output_brief(out).unwrap();
        assert!(b.contains("steps=12/12"), "{b}");
        assert!(b.contains("focus_lost=1"), "{b}");
        assert!(b.contains("retries=2"), "{b}");
        assert!(!b.contains("focus_aborted"), "{b}");
        // 止损中止
        let out = r#"{"ok": false, "action": "run_sequence", "steps_total": 7, "steps_ok": 4, "focus_lost_count": 3, "retried_steps": 0, "focus_aborted": true}"#;
        let b = window_use_output_brief(out).unwrap();
        assert!(b.contains("steps=4/7"), "{b}");
        assert!(b.contains("focus_aborted!"), "{b}");
    }
}
