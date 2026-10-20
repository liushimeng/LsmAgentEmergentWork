//! 页面观测:`MCP_Web_Use(inspect, info=...)` 的内核实现。
//!
//! 直接读取进程内 DOM/布局/网络台账,不经过任何外部浏览器协议。

use serde_json::{json, Value};

use super::document::{self, Document};
use super::style::{MediaCtx, StyleEngine};
use super::{EngineError, EngineResult, LsmBrowserManager};

fn p_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn p_u64(params: &Value, key: &str) -> Option<u64> {
    params.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

fn p_bool(params: &Value, key: &str, default: bool) -> bool {
    match params.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(default),
        _ => default,
    }
}

/// 执行一个 `info` 观测维度。
pub async fn inspect(
    mgr: &LsmBrowserManager,
    page_id: &str,
    info: &str,
    params: &Value,
) -> EngineResult<Value> {
    if page_id.is_empty() {
        return Err(EngineError::param("inspect 需要 page_id"));
    }
    let inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get(page_id)
        .ok_or_else(|| EngineError::page_not_found(page_id))?;
    let doc = Document::parse(&tab.html, &tab.url);

    match info {
        "console" => {
            let limit = p_u64(params, "limit").unwrap_or(100).min(500) as usize;
            let events: Vec<Value> = tab
                .console
                .iter()
                .rev()
                .take(limit)
                .rev()
                .map(|e| json!({"level": e.level, "text": e.text, "at_ms": e.at_ms}))
                .collect();
            Ok(json!({
                "events": events,
                "count": tab.console.len(),
                "collection_healthy": true,
                "last_event_at": tab.console.last().map(|e| e.at_ms),
            }))
        }
        "network" => {
            let limit = p_u64(params, "limit").unwrap_or(100).min(500) as usize;
            let events: Vec<Value> = tab
                .network
                .iter()
                .rev()
                .take(limit)
                .rev()
                .map(|r| {
                    json!({
                        "url": r.url, "method": r.method, "status": r.status,
                        "content_type": r.content_type, "bytes": r.bytes,
                        "elapsed_ms": r.elapsed_ms, "redirect": r.redirect, "at_ms": r.at_ms,
                    })
                })
                .collect();
            Ok(json!({
                "events": events,
                "count": tab.network.len(),
                "collection_healthy": true,
                "last_event_at": tab.network.last().map(|r| r.at_ms),
            }))
        }
        "elements" => inspect_elements(tab, &doc, params),
        "stylesheets" => {
            let engine = style_engine(tab, &doc);
            let sheets: Vec<Value> = engine
                .sheets()
                .iter()
                .enumerate()
                .map(|(idx, s)| {
                    json!({
                        "index": idx,
                        "href": s.href,
                        "media": s.media,
                        "bytes": s.bytes,
                        "rules": s.rules,
                    })
                })
                .collect();
            Ok(json!({
                "ok": true,
                "count": sheets.len(),
                "rules": engine.rule_count(),
                "sheets": sheets,
                "media": {
                    "width": engine.media().viewport_width,
                    "height": engine.media().viewport_height,
                    "prefers_dark": engine.media().prefers_dark,
                    "print": engine.media().print,
                },
                "note": "外链样式已由宿主预取并原位内联为 <style data-lsm-href=...>",
            }))
        }
        "dom" => inspect_dom(&doc, params),
        "localstorage" | "sessionstorage" => {
            let map = if info == "sessionstorage" {
                tab.session_storage.clone()
            } else {
                inner
                    .local_storage
                    .get(&super::urlutil::storage_origin(&tab.url))
                    .cloned()
                    .unwrap_or_default()
            };
            let prefix = p_str(params, "prefix").unwrap_or("");
            let contains = p_str(params, "contains").unwrap_or("");
            let filtered: serde_json::Map<String, Value> = map
                .into_iter()
                .filter(|(k, v)| {
                    (prefix.is_empty() || k.starts_with(prefix))
                        && (contains.is_empty() || v.contains(contains))
                })
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            Ok(Value::Object(filtered))
        }
        "cookies" => Ok(json!({
            "cookies": inner.cookies.all().iter().map(|c| c.to_json()).collect::<Vec<_>>(),
        })),
        "screenshot" | "ocr" => {
            let mut p = params.clone();
            if info == "ocr" {
                if let Value::Object(obj) = &mut p {
                    obj.insert("ocr".to_string(), Value::Bool(true));
                }
            }
            mgr.screenshot_locked(tab, &p)
        }
        "page_meta" => Ok(json!({
            "page_id": tab.id,
            "url": tab.url,
            "title": tab.title,
            "viewport": {
                "width": tab.viewport.width,
                "height": tab.viewport.height,
                "devicePixelRatio": tab.viewport.device_scale_factor,
                "mobile": tab.viewport.mobile,
            },
            "user_agent": tab.user_agent,
            "status": tab.status,
            "content_type": tab.content_type,
        })),
        "viewport" => {
            let scroll_height = tab.layout.page_height();
            Ok(json!({
                "width": tab.viewport.width,
                "height": tab.viewport.height,
                "devicePixelRatio": tab.viewport.device_scale_factor,
                "scrollX": tab.scroll_x,
                "scrollY": tab.scroll_y,
                "scrollWidth": tab.layout.width,
                "scrollHeight": scroll_height,
                "overflow": {
                    "horizontal": tab.layout.width > tab.viewport.width as f64,
                    "vertical": scroll_height > tab.viewport.height as f64,
                    "content_larger_than_viewport": scroll_height > tab.viewport.height as f64,
                },
                "next_action": if scroll_height > tab.viewport.height as f64 {
                    "内容高于视口,可用 control(scroll) 或 screenshot(full_page=true)"
                } else { "内容在视口内" },
            }))
        }
        "url" => Ok(json!({"url": tab.url})),
        "title" => Ok(json!({"title": tab.title})),
        "ping" => Ok(json!({"ok": true, "page_id": page_id, "latency_ms": 0, "ts": super::now_ms()})),
        "image_urls" => {
            let max = p_u64(params, "max").unwrap_or(20).min(100) as usize;
            let urls = doc.image_urls();
            // 阶段三第三步:附带解码状态与固有尺寸。
            let items: Vec<Value> = urls
                .iter()
                .take(max)
                .map(|u| {
                    let decoded = tab.images.get(u);
                    json!({
                        "url": u,
                        "loaded": decoded.is_some(),
                        "natural_width": decoded.map(|d| d.width),
                        "natural_height": decoded.map(|d| d.height),
                    })
                })
                .collect();
            Ok(json!({
                "img_count": urls.len(),
                "img_urls": urls.iter().take(max).cloned().collect::<Vec<_>>(),
                "data_image_count": urls.iter().filter(|u| u.starts_with("data:")).count(),
                "canvas_count": doc.select("canvas").map(|v| v.len()).unwrap_or(0),
                "svg_count": doc.select("svg").map(|v| v.len()).unwrap_or(0),
                "picture_count": doc.select("picture").map(|v| v.len()).unwrap_or(0),
                "decoded_count": tab.images.len(),
                "images": items,
                "note": "阶段三第三步:宿主预取并解码 <img>/背景图,解码结果仅存内存",
            }))
        }
        "blockers" => Ok(inspect_blockers(tab, &doc)),
        "visibility" => Ok(json!({
            "visibility": "visible",
            "focused": tab.foreground,
            "hidden": false,
            "note": "LsmAgentBrowser 进程内页面恒可见;无后台挂起",
        })),
        "extract_links" => inspect_links(&doc, params),
        "extract" => inspect_extract(&doc, params),
        "page_state" => inspect_page_state(&doc, params),
        "coverage" => Err(EngineError::param(
            "coverage 由兼容层基于访问台账提供,内核不直接处理",
        )),
        other => Err(EngineError::param(format!("未知 info:{other}"))),
    }
}

fn inspect_elements(
    tab: &super::TabState,
    doc: &Document,
    params: &Value,
) -> EngineResult<Value> {
    let selector = p_str(params, "selector").unwrap_or(
        "a[href], button, input, textarea, select, [role=\"button\"], [role=\"link\"], [onclick]",
    );
    let nodes = match doc.select(selector) {
        Ok(v) => v,
        Err(e) => {
            return Ok(json!({"ok": false, "count": 0, "index": 0, "error": e}));
        }
    };
    let idx = p_u64(params, "nth").unwrap_or(0) as usize;
    let count = nodes.len();
    let Some(node) = nodes.get(idx) else {
        return Ok(json!({"ok": false, "count": count, "index": idx}));
    };
    let stable = Document::stable_selector(node);
    let rect = tab.layout.rect_for(&stable);
    let mut v = doc.element_json(node, idx, count, rect, &tab.form_values);
    if !p_bool(params, "include_text", true) {
        if let Some(obj) = v.as_object_mut() {
            obj.remove("text");
        }
    }
    if p_bool(params, "include_outer_html", false) {
        v["outer_html"] = json!(document::outer_html(node));
    }
    // 可见性以计算样式为准(外链/内联 CSS 的 display/visibility 都生效)。
    let engine = style_engine(tab, doc);
    let computed = engine.computed_chain(node);
    let visible = computed.display != "none"
        && computed.visibility != "hidden"
        && computed.visibility != "collapse"
        && computed.opacity > 0.0;
    v["visible"] = json!(visible);
    if let Some(rect) = v.get_mut("rect").and_then(Value::as_object_mut) {
        rect.insert("visible".to_string(), json!(visible));
    }
    if p_bool(params, "include_style", false) {
        v["computed_style"] = json!(computed.to_map());
    }
    Ok(v)
}

/// 按标签页视口构建样式引擎(与布局使用同一份计算规则)。
fn style_engine(tab: &super::TabState, doc: &Document) -> StyleEngine {
    let media = MediaCtx::new(tab.viewport.width as f64, tab.viewport.height as f64);
    StyleEngine::from_document(doc, media)
}

fn inspect_dom(doc: &Document, params: &Value) -> EngineResult<Value> {
    let selector = p_str(params, "selector").unwrap_or("html");
    let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
    let Some(root) = nodes.first() else {
        return Ok(json!({"ok": false, "selector": selector}));
    };
    if selector == "html" {
        let max_depth = p_u64(params, "max_depth").unwrap_or(3).clamp(1, 10) as usize;
        let limit = p_u64(params, "node_count_limit").unwrap_or(2000).min(5000) as usize;
        let mut count = 0usize;
        let tree = dom_tree(root, 0, max_depth, limit, &mut count);
        Ok(json!({
            "ok": true,
            "selector": selector,
            "node_count": count,
            "tree": tree,
        }))
    } else {
        Ok(json!({
            "ok": true,
            "selector": selector,
            "outer_html": document::truncate_chars(&document::outer_html(root), 200_000),
        }))
    }
}

fn dom_tree(
    node: &kuchikiki::NodeRef,
    depth: usize,
    max_depth: usize,
    limit: usize,
    count: &mut usize,
) -> Value {
    if *count >= limit || depth > max_depth {
        return json!({ "truncated": true });
    }
    *count += 1;
    let tag = document::tag_name(node);
    let mut obj = json!({
        "tag": if tag.is_empty() { "#text".to_string() } else { tag.clone() },
    });
    if let Some(id) = document::attr(node, "id") {
        obj["id"] = json!(id);
    }
    if let Some(class) = document::attr(node, "class") {
        obj["class"] = json!(class);
    }
    if let Some(text) = node.as_text() {
        let t = text.borrow();
        if !t.trim().is_empty() {
            obj["text"] = json!(document::truncate_chars(t.trim(), 300));
        }
        return obj;
    }
    if depth < max_depth {
        let children: Vec<Value> = node
            .children()
            .map(|c| dom_tree(&c, depth + 1, max_depth, limit, count))
            .collect();
        obj["children"] = json!(children);
    }
    obj
}

fn inspect_links(doc: &Document, params: &Value) -> EngineResult<Value> {
    let selector = p_str(params, "selector").unwrap_or("a");
    let max = p_u64(params, "max_links").unwrap_or(200).min(500) as usize;
    let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
    let scanned = nodes.len();
    let mut links = Vec::new();
    for n in nodes.into_iter().take(max) {
        if let Some(v) = doc.link_json(&n) {
            links.push(v);
        }
    }
    Ok(json!({
        "links": links,
        "total": scanned,
        "truncated": scanned > max,
        "scanned": scanned,
        "hostname": super::urlutil::host_of(&doc.base_url),
    }))
}

fn inspect_extract(doc: &Document, params: &Value) -> EngineResult<Value> {
    if p_bool(params, "probe", false) {
        return Ok(probe_selectors(doc));
    }
    let item_selector = p_str(params, "item_selector")
        .ok_or_else(|| EngineError::param("extract 需要 item_selector 或 probe=true"))?;
    let items_nodes = doc
        .select(item_selector)
        .map_err(EngineError::action_failed)?;
    let scanned = items_nodes.len();
    let limit = p_u64(params, "limit").unwrap_or(200).min(1000) as usize;
    let fields = params
        .get("fields")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let url_from = p_str(params, "url_from").unwrap_or("a");
    let keywords: Vec<String> = params
        .get("filter")
        .and_then(|f| f.get("keywords"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.to_ascii_lowercase()).collect())
        .unwrap_or_default();

    let mut items = Vec::new();
    for node in items_nodes.into_iter() {
        if items.len() >= limit {
            break;
        }
        let item_text = document::collapse_ws(&node.text_contents());
        if !keywords.is_empty() {
            let lower = item_text.to_ascii_lowercase();
            if !keywords.iter().any(|k| lower.contains(k)) {
                continue;
            }
        }
        let mut obj = serde_json::Map::new();
        let mut missing_required = false;
        for (name, spec) in &fields {
            let sel = spec.get("selector").and_then(Value::as_str).unwrap_or("");
            let attr = spec.get("attr").and_then(Value::as_str);
            let required = spec.get("required").and_then(Value::as_bool).unwrap_or(false);
            let max_chars = spec
                .get("max_chars")
                .and_then(Value::as_u64)
                .unwrap_or(4000) as usize;
            let target = if sel.is_empty() {
                Some(node.clone())
            } else {
                node.select(sel).ok().and_then(|mut it| it.next().map(|n| n.as_node().clone()))
            };
            let value = target
                .as_ref()
                .map(|n| {
                    if let Some(a) = attr {
                        document::attr(n, a).unwrap_or_default()
                    } else {
                        document::collapse_ws(&n.text_contents())
                    }
                })
                .unwrap_or_default();
            if required && value.is_empty() {
                missing_required = true;
            }
            obj.insert(
                name.clone(),
                Value::String(document::truncate_chars(&value, max_chars)),
            );
        }
        if let Ok(link) = node.select(url_from) {
            if let Some(first) = link.into_iter().next() {
                if let Some(href) = document::attr(first.as_node(), "href") {
                    if let Some(abs) = super::urlutil::resolve(&doc.base_url, &href) {
                        obj.insert("url".to_string(), Value::String(abs));
                    }
                }
            }
        }
        if missing_required {
            continue;
        }
        items.push(Value::Object(obj));
    }
    let field_names: Vec<String> = fields.keys().cloned().collect();
    Ok(json!({
        "items": items,
        "total": scanned,
        "returned": items.len(),
        "scanned": scanned,
        "truncated": scanned > limit,
        "field_names": field_names,
        "hostname": super::urlutil::host_of(&doc.base_url),
        "hint": "阶段一按 CSS 选择器抽取;页面脚本渲染内容需阶段二支持",
    }))
}

fn probe_selectors(doc: &Document) -> Value {
    let nodes = doc
        .select("div, li, article, tr, section, a, span")
        .unwrap_or_default();
    let mut class_count: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for n in &nodes {
        if let Some(class) = document::attr(n, "class") {
            for c in class.split_whitespace() {
                *class_count.entry(c.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut repeated: Vec<(String, usize)> = class_count.into_iter().filter(|(_, c)| *c >= 3).collect();
    repeated.sort_by(|a, b| b.1.cmp(&a.1));
    let likely: Vec<String> = repeated
        .iter()
        .take(12)
        .map(|(c, _)| format!(".{c}"))
        .collect();
    let repeated_json: Vec<Value> = repeated
        .iter()
        .take(30)
        .map(|(c, n)| json!({"class": c, "count": n}))
        .collect();
    json!({
        "repeated_classes": repeated_json,
        "likely_item_classes": likely,
        "common_selectors": ["article", "li", ".item", ".card", "tr", "div[class]"],
        "candidate_count": nodes.len(),
    })
}

fn inspect_page_state(doc: &Document, params: &Value) -> EngineResult<Value> {
    let max_bytes = p_u64(params, "max_bytes").unwrap_or(20_000) as usize;
    let mut jsonld = Vec::new();
    for n in doc
        .select("script[type=\"application/ld+json\"]")
        .unwrap_or_default()
    {
        let text = n.text_contents();
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            jsonld.push(v);
        }
    }
    Ok(json!({
        "found": !jsonld.is_empty(),
        "globals": {},
        "window_globals": {},
        "jsonld": jsonld,
        "dropped_paths": [],
        "truncated": false,
        "hostname": super::urlutil::host_of(&doc.base_url),
        "max_bytes": max_bytes,
        "hint": "阶段一读取内联 JSON-LD;window 全局量需阶段二页面脚本执行",
    }))
}

fn inspect_blockers(tab: &super::TabState, doc: &Document) -> Value {
    let text = doc.text().to_ascii_lowercase();
    let checks: &[(&str, &[&str])] = &[
        ("captcha", &["验证码", "滑块", "人机验证", "captcha", "安全验证"]),
        ("login", &["请登录", "登录后查看", "sign in to continue"]),
        ("rate_limit", &["访问频繁", "访问过于频繁", "too many requests", "rate limit"]),
        ("risk", &["异常访问", "风控", "risk control"]),
    ];
    let mut blockers = Vec::new();
    for (kind, needles) in checks {
        for needle in needles.iter() {
            if text.contains(needle) {
                blockers.push(json!({
                    "kind": kind,
                    "evidence": needle,
                    "source": "page_text",
                }));
                break;
            }
        }
    }
    let blocked = !blockers.is_empty();
    json!({
        "blockers": blockers,
        "blocked": blocked,
        "suggested_action": if blocked { "request_human" } else { "continue" },
        "available_reasons": ["captcha", "sms", "qr_login", "login", "real_name", "two_factor", "oauth", "manual_verify", "custom"],
        "page_id": tab.id,
        "note": "阶段一按页面文本关键词判断;结构化弹层检测在阶段二实现",
    })
}
