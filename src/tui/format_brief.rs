//! TUI 工具输出摘要子模块(2026-09-18 第 88 轮新增)。
//!
//! 拆分背景:`format.rs` 已达 1700+ 临界线,按「单源码文件 ≤1800 行」规范,
//! 新增的 MCP_Window_Use 输出摘要函数落到本职责子模块;
//! 既有 `browser_output_brief` / `tool_args_brief` 仍在 format.rs(未机械搬移,
//! 后续轮次如需继续增长可整体迁入)。

use super::format::truncate_chars;

/// MCP_Web_Use `{code,message,data}` 信封的精简摘要。
///
/// ★ 2026-09-17 第 79 轮 P2-5 引入(format.rs);第 89 轮随 MCP_Web_Use 更名;
/// **第 99 轮迁入本子模块**(format.rs 已达临界线)+ 扩展:
/// - `save_path` → `out=<文件名>`、`byte_size` → `bytes=N`(screenshot/download 落盘);
/// - `reused=true` → `reused`(open 复用一眼可辨);
/// - `ocr_text` → `ocr_len=N`、`ocr_error` → `err=...`(验证码 OCR 结果);
/// - `result`(eval_js)→ `result=<头 24 字>`;`result_truncated` → 长度标注;
/// - `closed`(close all)→ `closed=N`。
/// 解析失败回退 None(调用方走普通截断)。
pub(crate) fn browser_output_brief(output_summary: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(output_summary.trim()).ok()?;
    if !v.is_object() {
        return None;
    }
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    let message = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
    let data = v.get("data").cloned().unwrap_or(serde_json::Value::Null);
    let mut parts: Vec<String> = vec![format!("code={code}")];
    if !message.is_empty() {
        parts.push(truncate_chars(message, 24));
    }
    if let Some(d) = data.as_object() {
        for key in ["page_id", "url", "title", "spawned_page_id"] {
            if let Some(s) = d.get(key).and_then(|x| x.as_str()) {
                if !s.is_empty() {
                    parts.push(format!("{key}={}", truncate_chars(s, 30)));
                }
            }
        }
        // 文本类字段只报长度(内容本身不该在 trace 区刷屏)
        for key in ["text", "outer_html", "markdown", "content"] {
            if let Some(s) = d.get(key).and_then(|x| x.as_str()) {
                let n = s.chars().count();
                if n > 0 {
                    parts.push(format!("{key}_len={n}"));
                }
            }
        }
        // ===== 第 99 轮扩展 =====
        if d.get("reused").and_then(|x| x.as_bool()) == Some(true) {
            parts.push("reused".to_string());
        }
        if let Some(n) = d.get("closed").and_then(|x| x.as_i64()) {
            parts.push(format!("closed={n}"));
        }
        if let Some(s) = d.get("save_path").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
            let name = std::path::Path::new(s)
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or(s);
            parts.push(format!("out={}", truncate_chars(name, 24)));
        }
        if let Some(n) = d.get("byte_size").and_then(|x| x.as_i64()) {
            parts.push(format!("bytes={n}"));
        }
        if let Some(s) = d.get("ocr_text").and_then(|x| x.as_str()) {
            let n = s.chars().count();
            if n > 0 {
                parts.push(format!("ocr_len={n}"));
            }
        }
        if let Some(s) = d.get("ocr_error").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
            parts.push(format!("ocr_err={}", truncate_chars(s, 30)));
        }
        match d.get("result") {
            Some(serde_json::Value::String(s)) => {
                parts.push(format!("result={}", truncate_chars(s, 24)));
            }
            Some(other @ (serde_json::Value::Number(_) | serde_json::Value::Bool(_))) => {
                parts.push(format!("result={other}"));
            }
            _ => {}
        }
        if d.get("result_truncated").and_then(|x| x.as_bool()) == Some(true) {
            let n = d.get("result_len").and_then(|x| x.as_i64()).unwrap_or(0);
            parts.push(format!("result=[已落盘 共{n}字符]"));
        }
    }
    Some(parts.join(" "))
}

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
pub(crate) fn window_use_output_brief(_action: &str, output_summary: &str) -> Option<String> {
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
    // 第 93 轮:纯 wait 批次(零 UI 动作)醒目标记 —— 纯等待不是完成证据。
    if get_b("wait_only") == Some(true) {
        parts.push("⚠wait_only".to_string());
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
        let b = window_use_output_brief("", out).expect("应能解析");
        assert!(b.contains("route=osascript_fallback"), "{b}");
        assert!(b.contains("verified=false"), "{b}");
        assert!(b.contains("frontmost=✓"), "{b}");
    }

    #[test]
    fn brief_chat_loop_rounds_and_focus_abort() {
        let out = r#"{"ok": true, "total_rounds": 5, "total_sent": 4, "total_replies": 1, "focus_aborted": true}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("rounds=5 sent=4 replies=1"), "{b}");
        assert!(b.contains("focus_aborted!"), "{b}");
    }

    #[test]
    fn brief_osascript_run_exit_code_with_stderr() {
        let out = r#"{"ok": false, "exit_code": 1, "stderr": "17:18: syntax error (-2741)\nmore"}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("exit=1"), "{b}");
        assert!(b.contains("syntax error"), "{b}");
        // 成功时不带 err
        let ok = window_use_output_brief("", r#"{"ok": true, "exit_code": 0}"#).unwrap();
        assert_eq!(ok, "exit=0");
    }

    #[test]
    fn brief_fallback_for_unparseable_or_empty() {
        assert!(window_use_output_brief("", "not json").is_none());
        assert!(window_use_output_brief("", "[]").is_none());
        // 纯 ok:true 无任何关键字段 → None(调用方回退截断)
        assert!(window_use_output_brief("", r#"{"ok": true}"#).is_none());
        // ocr blocks
        let b = window_use_output_brief("", r#"{"block_count": 42}"#).unwrap();
        assert_eq!(b, "blocks=42");
    }

    #[test]
    fn brief_input_batch_round90() {
        // 第 90 轮:input_batch 摘要 steps=成功/总数。
        let out = r#"{"ok": true, "action": "input_batch", "steps_total": 5, "steps_ok": 5}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("steps=5/5"), "{b}");
        // 部分失败
        let out = r#"{"ok": false, "action": "input_batch", "steps_total": 4, "steps_ok": 2}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("steps=2/4"), "{b}");
    }

    #[test]
    fn brief_run_sequence_round91() {
        // 第 91 轮:run_sequence 摘要 steps + focus_lost + retries。
        let out = r#"{"ok": true, "action": "run_sequence", "steps_total": 12, "steps_ok": 12, "focus_lost_count": 1, "retried_steps": 2, "focus_aborted": false}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("steps=12/12"), "{b}");
        assert!(b.contains("focus_lost=1"), "{b}");
        assert!(b.contains("retries=2"), "{b}");
        assert!(!b.contains("focus_aborted"), "{b}");
        // 止损中止
        let out = r#"{"ok": false, "action": "run_sequence", "steps_total": 7, "steps_ok": 4, "focus_lost_count": 3, "retried_steps": 0, "focus_aborted": true}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("steps=4/7"), "{b}");
        assert!(b.contains("focus_aborted!"), "{b}");
    }

    #[test]
    fn brief_run_sequence_wait_only_round93() {
        // 第 93 轮:纯 wait 批次(零 UI 动作)摘要带 ⚠wait_only 警示。
        let out = r#"{"ok": true, "action": "run_sequence", "steps_total": 18, "steps_ok": 16, "wait_only": true, "focus_lost_count": 0, "retried_steps": 2, "focus_aborted": false}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(b.contains("steps=16/18"), "{b}");
        assert!(b.contains("⚠wait_only"), "{b}");
        // 有 UI 动作的批次不带警示
        let out = r#"{"ok": true, "action": "run_sequence", "steps_total": 6, "steps_ok": 6, "wait_only": false}"#;
        let b = window_use_output_brief("", out).unwrap();
        assert!(!b.contains("wait_only"), "{b}");
    }
}

#[cfg(test)]
mod browser_output_brief_tests {
    use super::*;

    #[test]
    fn brief_reused_and_page() {
        let out = r#"{"code":0,"message":"ok","data":{"page_id":"p_a1b2c3d4","reused":true,"final_url":"http://10.0.0.1:20122/"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("reused"), "{b}");
        assert!(b.contains("page_id=p_a1b2c3d4"), "{b}");
    }

    #[test]
    fn brief_screenshot_save_path_and_bytes() {
        let out = r#"{"code":0,"message":"ok","data":{"save_path":"/tmp/laew/captcha.png","byte_size":104527,"format":"png"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("out=captcha.png"), "{b}");
        assert!(b.contains("bytes=104527"), "{b}");
    }

    #[test]
    fn brief_ocr_len_not_content() {
        let out = r#"{"code":0,"message":"ok","data":{"ocr_text":"4a7c","ocr_block_count":4,"save_path":"/tmp/x.png"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("ocr_len=4"), "{b}");
        assert!(!b.contains("4a7c"), "验证码内容不应进摘要: {b}");
    }

    #[test]
    fn brief_eval_js_result_head_and_truncated() {
        let out = r#"{"code":0,"message":"ok","data":{"result":"https://example.com/page"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("result=https://example.com"), "{b}");
        // 超长落盘
        let out = r#"{"code":0,"message":"ok","data":{"result_truncated":true,"result_len":140000,"saved_to":"/tmp/laew_web_result_1.png"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("result=[已落盘 共140000字符]"), "{b}");
    }

    #[test]
    fn brief_close_all_count() {
        let out = r#"{"code":0,"message":"closed","data":{"page_id":"all","closed":3}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("closed=3"), "{b}");
    }

    #[test]
    fn brief_data_envelope_still_works() {
        // 存量行为回归:文本类字段只报长度
        let out = r#"{"code":0,"message":"ok","data":{"page_id":"p_1","text":"一二三四五"}}"#;
        let b = browser_output_brief(out).unwrap();
        assert!(b.contains("text_len=5"), "{b}");
    }
}
