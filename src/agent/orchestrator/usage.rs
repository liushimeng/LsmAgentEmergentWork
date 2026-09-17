//! 用量与参数摘要辅助(2026-09-17 自 orchestrator.rs 拆分)。

use super::*;


/// 2026-09-16 第 67 轮:工具参数摘要(供 [laew] 工具调用明细行)。
///
/// 从 ToolCallLogEntry.args_json(稳定序列化)提取关键字段拼 `k=v` 列表:
/// query / filter / window_id / path / action / text / x / y / command / url / selector,
/// 截 60 字符 —— 微信视觉路线复盘时能直接看到「点了哪个坐标 / 输了什么文本」。
pub(super) fn tool_args_digest(args_json: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return String::new();
    };
    let Some(obj) = v.as_object() else {
        return String::new();
    };
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
