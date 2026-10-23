//! `MCP_Web_LsmAgentBrowser_Use(action=inspect, info=...)` 兼容层。
//!
//! page_id 校验、参数归一化、coverage 台账、`page_dead` 标注;观测本身由
//! `agent::browser::inspect` 在进程内完成。

use serde_json::{json, Value};

use super::{engine_error, envelope, str_arg, McpWebLsmBrowserUseTool};
use crate::agent::browser;
use crate::error::Result;

pub(super) async fn run(args: Value) -> Result<String> {
    let Some(page_id) = str_arg(&args, "page_id").map(str::to_string) else {
        return envelope(1001, "inspect 需要 page_id", json!({}));
    };
    let Some(info) = str_arg(&args, "info").map(str::to_string) else {
        return envelope(
            1001,
            "inspect 需要 info",
            json!({"allowed": [
                "console","network","elements","dom","localstorage","sessionstorage","cookies",
                "screenshot","page_meta","viewport","url","title","ping","image_urls","ocr",
                "blockers","visibility","extract_links","extract","page_state","coverage",
                "text","page_health","stylesheets"
            ]}),
        );
    };
    // coverage 是进程级遍历台账,不依赖存活页面(与旧契约一致,仍要求 page_id 语法)。
    if info == "coverage" {
        return envelope(0, "ok", super::visit_ledger::coverage_payload());
    }
    let mgr = McpWebLsmBrowserUseTool::mgr();
    if info != "ping" && !mgr.page_exists(&page_id).await {
        return envelope(2000, "page_id 无效或页面已关闭", json!({"page_id": page_id}));
    }
    let params = normalize_inspect_params(&args);
    match browser::inspect::inspect(&mgr, &page_id, &info, &params).await {
        Ok(mut data) => {
            if !data.is_object() {
                data = json!({"result": data});
            }
            data["page_id"] = json!(page_id);
            if info != "screenshot" && info != "ocr" {
                if let Some(p) = mgr.page_summary(&page_id).await {
                    if p.url.trim().eq_ignore_ascii_case("about:blank") {
                        data["page_dead"] = json!({
                            "dead": true,
                            "landed_url": p.url,
                            "hint": "页面落在 about:blank:可重新 open,或如实报告站点不可达",
                        });
                    }
                }
            }
            envelope(0, "ok", data)
        }
        Err(e) => Ok(engine_error(e)),
    }
}

/// 顶层便捷字段并入 params(LLM 常把 selector/nth/limit 写在顶层)。
fn normalize_inspect_params(args: &Value) -> Value {
    let mut obj = match args.get("params").cloned() {
        Some(Value::Object(o)) => o,
        Some(Value::String(s)) => serde_json::from_str::<Value>(&s)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default(),
        _ => serde_json::Map::new(),
    };
    for key in [
        "selector", "nth", "limit", "include_text", "include_outer_html", "max_depth",
        "node_count_limit", "prefix", "contains", "max", "max_links", "include_context",
        "context_length", "item_selector", "fields", "url_from", "scan_cap", "filter",
        "probe", "keys", "probe_window", "max_bytes", "max_array_items", "save_path",
        "output_path", "path", "clip",
        "full_page", "ocr", "region", "return_base64", "format", "quality", "state",
        "timeout_ms",
    ] {
        if let Some(v) = args.get(key) {
            obj.entry(key.to_string()).or_insert_with(|| v.clone());
        }
    }
    Value::Object(obj)
}
