//! 用量与参数摘要辅助(2026-09-17 自 orchestrator.rs 拆分)。

use super::*;


/// 2026-09-16 第 67 轮:工具参数摘要(供 [laew] 工具调用明细行)。
///
/// 从 ToolCallLogEntry.args_json(稳定序列化)提取关键字段拼 `k=v` 列表:
/// query / filter / window_id / path / action / text / x / y / command / url / selector,
/// 截 60 字符 —— 微信视觉路线复盘时能直接看到「点了哪个坐标 / 输了什么文本」。
///
/// 2026-09-17 第 74 轮:差异化处理 Browser* / Bash / Read / Write / Edit / Glob / Grep 工具,
/// 在 tool 前缀独立标识, 减少 TUI 信息密度(用户主诉「输出太多看不清」):
/// - BrowserControl:action=input_text selector=X text=Y / action=click selector=X / action=wait selector=X
/// - BrowserInspect:info=elements selector=X
/// - BrowserNew:url=X
/// - Bash:command=X(截 40)
/// - Read/Write/Edit:path=X(截 50)
/// - Glob:pattern=X
/// - Grep:pattern=X
pub(super) fn tool_args_digest(tool_name: &str, args_json: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return String::new();
    };
    let Some(obj) = v.as_object() else {
        return String::new();
    };
    // 工具级差异化分支(2026-09-17 第 74 轮)
    match tool_name {
        "BrowserControl" => {
            let action = obj.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let mut parts: Vec<String> = Vec::new();
            parts.push(format!("action={}", action));
            if let Some(sel) = obj.get("selector").and_then(|v| v.as_str()) {
                if !sel.is_empty() {
                    parts.push(format!("sel={}", truncate_progress_text(sel, 24)));
                }
            }
            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    parts.push(format!("text={}", truncate_progress_text(text, 30)));
                }
            }
            if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
                if !url.is_empty() {
                    parts.push(format!("url={}", truncate_progress_text(url, 30)));
                }
            }
            if let Some(x) = obj.get("x").and_then(|v| v.as_f64()) {
                parts.push(format!("x={:.0}", x));
            }
            if let Some(y) = obj.get("y").and_then(|v| v.as_f64()) {
                parts.push(format!("y={:.0}", y));
            }
            if let Some(ms) = obj.get("timeout_ms").and_then(|v| v.as_u64()) {
                parts.push(format!("timeout={}ms", ms));
            }
            // 2026-09-17 第 76 轮:扩展关键字段(file_paths / event / source_selector
            // / target_selector / value / index / keys / keys 数组),覆盖新增 actions。
            if let Some(files) = obj.get("file_paths").and_then(|v| v.as_array()) {
                if !files.is_empty() {
                    let joined = files
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| truncate_progress_text(s, 16))
                        .collect::<Vec<_>>()
                        .join(",");
                    parts.push(format!("files=[{}]({})", joined, files.len()));
                }
            }
            if let Some(file) = obj.get("file_path").and_then(|v| v.as_str()) {
                if !file.is_empty() {
                    parts.push(format!("file={}", truncate_progress_text(file, 30)));
                }
            }
            if let Some(event) = obj.get("event").and_then(|v| v.as_str()) {
                if !event.is_empty() {
                    parts.push(format!("event={event}"));
                }
            }
            if let Some(ssel) = obj.get("source_selector").and_then(|v| v.as_str()) {
                if !ssel.is_empty() {
                    parts.push(format!("src={}", truncate_progress_text(ssel, 18)));
                }
            }
            if let Some(tsel) = obj.get("target_selector").and_then(|v| v.as_str()) {
                if !tsel.is_empty() {
                    parts.push(format!("dst={}", truncate_progress_text(tsel, 18)));
                }
            }
            if let Some(value) = obj.get("value").and_then(|v| v.as_str()) {
                if !value.is_empty() {
                    parts.push(format!("value={}", truncate_progress_text(value, 20)));
                }
            }
            if let Some(idx) = obj.get("index").and_then(|v| v.as_i64()) {
                parts.push(format!("idx={idx}"));
            }
            if let Some(keys) = obj.get("keys").and_then(|v| v.as_array()) {
                let joined: Vec<String> = keys
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect();
                if !joined.is_empty() {
                    parts.push(format!("keys={}", truncate_progress_text(&joined.join("+"), 24)));
                }
            } else if let Some(keys_str) = obj.get("keys").and_then(|v| v.as_str()) {
                if !keys_str.is_empty() {
                    parts.push(format!("keys={}", truncate_progress_text(keys_str, 24)));
                }
            }
            // drag / mouse_move 坐标对
            if let Some(sx) = obj.get("source_x").and_then(|v| v.as_f64()) {
                parts.push(format!("sx={:.0}", sx));
            }
            if let Some(sy) = obj.get("source_y").and_then(|v| v.as_f64()) {
                parts.push(format!("sy={:.0}", sy));
            }
            if let Some(tx) = obj.get("target_x").and_then(|v| v.as_f64()) {
                parts.push(format!("tx={:.0}", tx));
            }
            if let Some(ty) = obj.get("target_y").and_then(|v| v.as_f64()) {
                parts.push(format!("ty={:.0}", ty));
            }
            truncate_progress_text(&parts.join(" "), 100)
        }
        "BrowserInspect" => {
            let info = obj.get("info").and_then(|v| v.as_str()).unwrap_or("");
            let mut parts: Vec<String> = vec![format!("info={}", info)];
            if let Some(sel) = obj.get("selector").and_then(|v| v.as_str()) {
                if !sel.is_empty() {
                    parts.push(format!("sel={}", truncate_progress_text(sel, 24)));
                }
            }
            if let Some(params) = obj.get("params").and_then(|v| v.as_object()) {
                if let Some(t) = params.get("timeout_ms").and_then(|v| v.as_u64()) {
                    parts.push(format!("timeout={}ms", t));
                }
                if let Some(m) = params.get("max").and_then(|v| v.as_u64()) {
                    parts.push(format!("max={}", m));
                }
            }
            truncate_progress_text(&parts.join(" "), 80)
        }
        "BrowserNew" => {
            let url = obj.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let mode = obj.get("mode").and_then(|v| v.as_str()).unwrap_or("hidden");
            truncate_progress_text(&format!("mode={mode} url={}", truncate_progress_text(url, 40)), 80)
        }
        "BrowserList" | "BrowserClose" | "BrowserOpen" => {
            let pid = obj.get("page_id").and_then(|v| v.as_str()).unwrap_or("");
            truncate_progress_text(&format!("page_id={}", truncate_progress_text(pid, 20)), 60)
        }
        "Bash" => {
            let cmd = obj.get("command").and_then(|v| v.as_str()).unwrap_or("");
            truncate_progress_text(&format!("cmd={}", truncate_progress_text(cmd, 40)), 80)
        }
        "Read" | "Write" | "Edit" => {
            let path = obj.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            truncate_progress_text(&format!("path={}", truncate_progress_text(path, 50)), 80)
        }
        "Glob" => {
            let pat = obj.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            truncate_progress_text(&format!("pattern={}", truncate_progress_text(pat, 40)), 80)
        }
        "Grep" => {
            let pat = obj.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = obj.get("path").and_then(|v| v.as_str()).unwrap_or("");
            truncate_progress_text(
                &format!("pattern={} path={}", truncate_progress_text(pat, 30), truncate_progress_text(path, 30)),
                80,
            )
        }
        "WindowFind" | "WindowInspect" | "WindowList" | "WindowOpen" | "WindowOCR" | "WindowScreenshot" | "WindowAction" => {
            // 复用原通用 KEYS,提取最关键的几个字段
            let mut parts = Vec::new();
            for k in ["window_id", "query", "selector", "action"] {
                if let Some(val) = obj.get(k) {
                    let vs = match val {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if !vs.is_empty() {
                        parts.push(format!("{k}={}", truncate_progress_text(&vs, 20)));
                    }
                }
            }
            truncate_progress_text(&parts.join(" "), 60)
        }
        _ => {
            // 通用 fallback: 提取已知的几个关键字段
            const KEYS: &[&str] = &[
                "query", "filter", "window_id", "path", "action", "text", "x", "y", "command", "url",
                "selector", "max_depth", "lang",
            ];
            let mut parts = Vec::new();
            for k in KEYS {
                if let Some(val) = obj.get(*k) {
                    let vs = match val {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if vs.is_empty() {
                        continue;
                    }
                    parts.push(format!("{k}={}", truncate_progress_text(&vs, 24)));
                }
            }
            truncate_progress_text(&parts.join(" "), 60)
        }
    }
}

pub(super) fn add_usage(mut total: Usage, delta: Usage) -> Usage {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.cache_read_input_tokens = total
        .cache_read_input_tokens
        .saturating_add(delta.cache_read_input_tokens);
    total.cache_creation_input_tokens = total
        .cache_creation_input_tokens
        .saturating_add(delta.cache_creation_input_tokens);
    total
}

pub(super) fn failure_usage(_failure: &QualityFailure) -> Usage {
    // QC fail 时由 QualityFailure 携带 SubAgent + QC 用量;Agent 错误路径为默认零。
    // 外层调用方只累加一次,避免与下一次执行层返回值重复计算。
    _failure.usage.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-17 第 74 轮:tool_args_digest 差异化测试。

    #[test]
    fn tool_args_digest_browser_control_input_text() {
        let args = r#"{"page_id":"p_abc","action":"input_text","selector":"textarea","text":"hello world"}"#;
        let s = tool_args_digest("BrowserControl", args);
        assert!(s.contains("action=input_text"), "action 应被包含: {s}");
        assert!(s.contains("sel=textarea"), "selector 应被包含: {s}");
        assert!(s.contains("text=hello world"), "text 应被包含: {s}");
    }

    #[test]
    fn tool_args_digest_browser_control_click() {
        let args = r#"{"page_id":"p_abc","action":"click","selector":"button.submit"}"#;
        let s = tool_args_digest("BrowserControl", args);
        assert!(s.contains("action=click"));
        assert!(s.contains("sel=button.submit"));
        assert!(!s.contains("text="), "click 不应带 text");
    }

    #[test]
    fn tool_args_digest_browser_inspect_elements() {
        let args = r#"{"page_id":"p_abc","info":"elements","selector":".response"}"#;
        let s = tool_args_digest("BrowserInspect", args);
        assert!(s.contains("info=elements"));
        assert!(s.contains("sel=.response"));
    }

    #[test]
    fn tool_args_digest_browser_new() {
        let args = r#"{"url":"https://wenxin.baidu.com/","mode":"hidden"}"#;
        let s = tool_args_digest("BrowserNew", args);
        assert!(s.contains("mode=hidden"));
        assert!(s.contains("url=https://wenxin.baidu.com/"));
    }

    #[test]
    fn tool_args_digest_bash() {
        let args = r#"{"command":"git status"}"#;
        let s = tool_args_digest("Bash", args);
        assert!(s.contains("cmd=git status"));
    }

    #[test]
    fn tool_args_digest_read_write_edit() {
        let args = r#"{"file_path":"/tmp/foo.txt"}"#;
        let s = tool_args_digest("Read", args);
        assert!(s.contains("path=/tmp/foo.txt"));
        let s = tool_args_digest("Write", args);
        assert!(s.contains("path=/tmp/foo.txt"));
        let s = tool_args_digest("Edit", args);
        assert!(s.contains("path=/tmp/foo.txt"));
    }

    #[test]
    fn tool_args_digest_glob_grep() {
        let args = r#"{"pattern":"*.rs"}"#;
        let s = tool_args_digest("Glob", args);
        assert!(s.contains("pattern=*.rs"));
        let args = r#"{"pattern":"fn main","path":"src/"}"#;
        let s = tool_args_digest("Grep", args);
        assert!(s.contains("pattern=fn main"));
        assert!(s.contains("path=src/"));
    }

    #[test]
    fn tool_args_digest_unknown_tool_falls_back() {
        let args = r#"{"query":"hello","x":100,"y":200}"#;
        let s = tool_args_digest("UnknownTool", args);
        assert!(s.contains("query=hello"));
        assert!(s.contains("x=100"));
        assert!(s.contains("y=200"));
    }

    #[test]
    fn tool_args_digest_truncates_long_values() {
        let long = "x".repeat(200);
        let args = format!(r#"{{"command":"{long}"}}"#);
        let s = tool_args_digest("Bash", &args);
        assert!(s.len() <= 80, "摘要应被截断到 80 字符内,实际 {} 字符", s.len());
        assert!(s.starts_with("cmd="));
    }

    #[test]
    fn tool_args_digest_handles_invalid_json() {
        assert_eq!(tool_args_digest("Bash", "not json"), "");
        assert_eq!(tool_args_digest("Read", ""), "");
    }

    #[test]
    fn tool_args_digest_handles_non_object() {
        assert_eq!(tool_args_digest("Bash", "[]"), "");
        assert_eq!(tool_args_digest("Bash", "null"), "");
        assert_eq!(tool_args_digest("Bash", "42"), "");
    }
}
