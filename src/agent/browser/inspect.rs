//! 页面观测:`MCP_Web_LsmAgentBrowser_Use(inspect, info=...)` 的内核实现。
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
    let mut inner = mgr.inner.lock().await;
    // 「快照×布局」一致性守卫:html 在上次布局后被脚本改写时先重排,
    // 保证 rect_for/page_health 等读数与当前 DOM 一致。
    // 需要 &mut,放独立作用域,之后降级为不可变借用(arms 里还会读 inner 的
    // http_cache/local_storage 等兄弟字段)。
    {
        let Some(tab) = inner.tabs.get_mut(page_id) else {
            return Err(EngineError::page_not_found(page_id));
        };
        mgr.ensure_layout_locked(tab);
    }
    let tab = inner
        .tabs
        .get(page_id)
        .ok_or_else(|| EngineError::page_not_found(page_id))?;
    let doc = tab.document_with_state();

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
                        "elapsed_ms": r.elapsed_ms, "redirect": r.redirect,
                        "from_cache": r.from_cache, "at_ms": r.at_ms,
                    })
                })
                .collect();
            let stats = inner.http_cache.stats();
            Ok(json!({
                "events": events,
                "count": tab.network.len(),
                "collection_healthy": true,
                "last_event_at": tab.network.last().map(|r| r.at_ms),
                "cache_stats": {
                    "hits": stats.hits,
                    "misses": stats.misses,
                    "size_bytes": stats.size_bytes,
                    "entry_count": stats.entry_count,
                },
            }))
        }
        "elements" => inspect_elements(tab, &doc, params),
        // `html`:整页 HTML 源码(工具描述里一直宣传该维度,但枚举此前没有它,
        // 模型按描述调用会被参数校验拒绝 —— 实测猎聘任务白费三轮 LLM 往返)。
        "html" => {
            let max = p_u64(params, "max_bytes").unwrap_or(200_000).min(2_000_000) as usize;
            let sel = p_str(params, "selector");
            let html = match sel {
                Some(s) if !s.is_empty() && s != "html" => {
                    let nodes = doc.select(s).map_err(EngineError::action_failed)?;
                    match nodes.first() {
                        Some(n) => document::outer_html(n),
                        None => String::new(),
                    }
                }
                _ => doc.serialize_full(),
            };
            Ok(json!({
                "ok": true,
                "selector": sel.unwrap_or("html"),
                "byte_size": html.len(),
                "truncated": html.len() > max,
                "html": document::truncate_chars(&html, max),
            }))
        }
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
            "webgl_requested": tab.webgl_asked,
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
        "text" => {
            // 页面可见文本:优先取布局产出的文本行(与截图内容一致),
            // 布局无文本时回退到 DOM 文本抽取。
            let limit = p_u64(params, "limit").unwrap_or(20_000).min(200_000) as usize;
            let layout_text = tab
                .layout
                .lines
                .iter()
                .filter(|l| l.visible)
                .map(|l| l.text.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let (text, source) = if layout_text.trim().is_empty() {
                (document::collapse_ws(&doc.text()), "dom")
            } else {
                (layout_text, "layout")
            };
            let total = text.chars().count();
            let clipped = document::truncate_chars(&text, limit);
            // 页面可访问文本极少(画布/图标型 Web App:百度地图整页 DOM 文本仅 4 字)
            // 时,追加「控件可访问名清单」—— Agent 由此知道页面上有哪些**可操作**
            // 控件及其语义(`data-title`/`title`/`aria-label`/`placeholder`),
            // 而不必靠猜 class 名反复试探。
            let controls = if total < 200 {
                let mut out: Vec<Value> = Vec::new();
                for n in doc.interactive_nodes().into_iter().take(40) {
                    let attrs = document::common_attrs(&n);
                    let label = document::accessible_name(&n, &attrs, None);
                    if label.is_empty() {
                        continue;
                    }
                    out.push(json!({
                        "selector": Document::stable_selector(&n),
                        "tag": document::tag_name(&n),
                        "label": label,
                        "visible": document::is_visible(&n),
                    }));
                    if out.len() >= 24 {
                        break;
                    }
                }
                out
            } else {
                Vec::new()
            };
            Ok(json!({
                "page_id": page_id,
                "text": clipped,
                "source": source,
                "chars": total,
                "returned_chars": clipped.chars().count(),
                "truncated": total > clipped.chars().count(),
                "controls": controls,
                "note": "source=layout 表示这是截图里能看到的文字;source=dom 表示布局层没有产出文本(元素被 display:none 剪枝或页面内容由 JS 生成)。controls 在页面文本极少时列出可操作控件的可访问名(label 来自 aria-label/data-title/title/alt/placeholder)",
            }))
        }
        "page_health" => {
            // 页面健康度:把"渲染空白"这类症状翻译成可诊断的结论,
            // 避免 Agent 在一张空白截图前反复重试。
            let dom_nodes = doc.select("*").map(|v| v.len()).unwrap_or(0);
            let iframe_count = doc.select("iframe").map(|v| v.len()).unwrap_or(0);
            let layout_text_chars = tab
                .layout
                .lines
                .iter()
                .filter(|l| l.visible)
                .map(|l| l.text.chars().count())
                .sum::<usize>();
            let visible_boxes = tab.layout.boxes.iter().filter(|b| b.visible).count();
            let errors: Vec<&super::ConsoleRecord> =
                tab.console.iter().filter(|c| c.level == "error").collect();
            let blank = layout_text_chars == 0 && visible_boxes == 0;
            // 能力缺口:站点申请过 WebGL 上下文(百度地图等)→ 内核无 GPU 后端,
            // 该区域画不出像素。这是**内核边界**而非页面故障,如实说明,免得
            // Agent 对同一片空白反复截图/重试。
            let webgl_gap = tab.webgl_asked > 0;
            // 第 174 轮:脚本管线截断也是一等健康信号 —— 截断意味着懒加载组件
            // (登录弹层/搜索面板)可能还没挂载,先补齐脚本再判断页面状态。
            let script_cut = tab.script_budget_cut && tab.script_pending > 0;
            // 同路径请求循环探测:验证码组件反复重置 / 轮询打转会在网络台账留下
            // 「同 method+路径(忽略 query)大量重复」的形态(实测 zhipin 安全验证页
            // GeeTest reset.php 连打 19 次直到脚本预算截断,页面只显示「配置错误」)。
            // 这是一等健康信号:让 Agent 直接看到「组件在自愈循环」,而不是对着
            // 「配置错误」文本反复刷新。仅诊断,不改行为(正常长轮询站点也会命中,
            // 文案用「疑似」,让模型结合 console/network 自行判断)。
            let mut loop_counts: std::collections::HashMap<(String, String), usize> =
                std::collections::HashMap::new();
            for ev in tab.network.iter().rev().take(500) {
                // 归一键:去 scheme 与 query,保留 host+path(不同域同路径不混淆;
                // 相对 URL 原样保留路径)。
                let host_path = ev
                    .url
                    .split_once("://")
                    .map(|(_, rest)| rest)
                    .unwrap_or(ev.url.as_str());
                let host_path = host_path
                    .split_once('?')
                    .map(|(p, _)| p)
                    .unwrap_or(host_path);
                *loop_counts
                    .entry((ev.method.clone(), host_path.to_string()))
                    .or_insert(0) += 1;
            }
            let request_loop = loop_counts
                .into_iter()
                .filter(|(_, n)| *n >= 8)
                .max_by_key(|(k, n)| (*n, k.1.clone()))
                .map(|((method, path), n)| {
                    json!({"method": method, "path": path, "count": n})
                });
            let (diagnosis, hint) = if script_cut {
                (
                    "页面脚本管线被墙钟预算截断,仍有模块未装载,动态组件可能尚未挂载",
                    "这是加载中状态而非页面故障:先 control(wait) 1-3 秒或再次执行一次轻量 eval_js 触发补齐,脚本会从上次断点继续;若持续截断可提高 LAEW_WEB_SCRIPT_INTERACT_MS",
                )
            } else if !errors.is_empty() {
                if blank {
                    (
                        "页面脚本执行失败:主内容由 JS 生成,内核脚本运行时未能完成初始化,导致内容节点保持初始隐藏态或未被创建",
                        "先用 inspect(console) 看具体报错;若目标站点重度依赖 JS 渲染,应改用该站点的非 JS 入口(如静态接口页/服务端渲染页)完成取证",
                    )
                } else {
                    (
                        "页面已渲染出部分内容,但脚本执行存在错误,动态区域可能尚未完成初始化",
                        "先用 inspect(console) 与 inspect(network) 定位脚本错误;静态页头/页脚可见不代表动态主内容已就绪",
                    )
                }
            } else if webgl_gap {
                (
                    "页面已渲染出 DOM,站点申请过 WebGL 上下文;对应画布区域可能无像素",
                    "内核无 WebGL:画布/三维区域画不出来。需要这些区域的像素时请改用 MCP_Web_Chrome_Use(真 Chromium);本工具仍可读页面 DOM 文本。",
                )
            } else if !blank {
                ("页面已渲染出内容", "")
            } else {
                (
                    "页面内容为空或全部被 display:none 隐藏(内核未渲染出任何可见内容)",
                    "用 inspect(elements) 逐层查祖先是否有 display:none;用 inspect(dom) 看静态 HTML 是否包含目标节点",
                )
            };
            Ok(json!({
                "page_id": page_id,
                "url": tab.url,
                "dom_nodes": dom_nodes,
                "layout_boxes": tab.layout.boxes.len(),
                "visible_boxes": visible_boxes,
                "layout_lines": tab.layout.lines.len(),
                "visible_text_chars": layout_text_chars,
                "page_height": tab.layout.page_height(),
                "console_events": tab.console.len(),
                "console_errors": errors.len(),
                "first_errors": errors.iter().take(3).map(|e| e.text.clone()).collect::<Vec<_>>(),
                "blank_page": blank,
                "webgl_requested": tab.webgl_asked,
                "iframe_count": iframe_count,
                "iframe_note": if iframe_count > 0 {
                    "内核不渲染 iframe 内容(只保留占位几何);iframe 内登录/验证码请顶层 navigate 到其 src 或改用 MCP_Web_Chrome_Use"
                } else {
                    ""
                },
                "script_budget_cut": tab.script_budget_cut,
                "script_pending": tab.script_pending,
                "request_loop": request_loop,
                "request_loop_note": if request_loop.is_some() {
                    "同路径请求大量重复(疑似验证码组件反复重置/轮询循环):结合 inspect(console) 的异常看组件是否在自愈循环;刷新通常无济于事,必要时改用 MCP_Web_Chrome_Use 或 request_human"
                } else {
                    ""
                },
                "diagnosis": diagnosis,
                "hint": hint,
            }))
        }
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
    let selector = p_str(params, "selector").unwrap_or(document::INTERACTIVE_DEFAULT);
    let nodes = match doc.select(selector) {
        Ok(v) => v,
        Err(e) => {
            return Ok(json!({"ok": false, "count": 0, "index": 0, "error": e}));
        }
    };
    // 列表模式:一次返回全部匹配元素(此前只回 `nth` 命中的**单个**元素 +
    // count,模型想看「有哪些匹配项」只能一轮一个元素地试 —— 实测猎聘任务
    // 连续多轮 inspect(elements) 都只拿到一个元素,反复误判「弹层不存在」)。
    if p_bool(params, "list", false) {
        let limit = p_u64(params, "max").unwrap_or(60).clamp(1, 500) as usize;
        let engine = style_engine(tab, doc);
        let mut items = Vec::new();
        for (i, node) in nodes.iter().enumerate() {
            if i >= limit {
                break;
            }
            let stable = Document::stable_selector(node);
            let rect = tab.layout.rect_for(&stable);
            let mut v = doc.element_json(
                node,
                i,
                nodes.len(),
                rect,
                &tab.form_values,
                &tab.pseudo_states,
                tab.focused.as_deref(),
            );
            let computed = engine.computed_chain(node);
            let visible = engine.is_rendered(node)
                && computed.display != "none"
                && computed.visibility != "hidden"
                && computed.visibility != "collapse"
                && computed.opacity > 0.0;
            v["visible"] = json!(visible);
            if let Some(obj) = v.get_mut("rect").and_then(Value::as_object_mut) {
                obj.insert("visible".to_string(), json!(visible));
                obj.insert("laid_out".to_string(), json!(rect.is_some()));
            }
            items.push(v);
        }
        return Ok(json!({
            "ok": true,
            "mode": "list",
            "selector": selector,
            "count": nodes.len(),
            "returned": items.len(),
            "truncated": nodes.len() > items.len(),
            "items": items,
            "hint": "list=true 返回全部匹配元素;每项含 tag/id/class/rect/text/visible",
        }));
    }
    let idx = p_u64(params, "nth").unwrap_or(0) as usize;
    let count = nodes.len();
    let Some(node) = nodes.get(idx) else {
        return Ok(json!({"ok": false, "count": count, "index": idx}));
    };
    let stable = Document::stable_selector(node);
    let rect = tab.layout.rect_for(&stable);
    let rect_found = rect.is_some();
    let mut v = doc.element_json(
        node,
        idx,
        count,
        rect,
        &tab.form_values,
        &tab.pseudo_states,
        tab.focused.as_deref(),
    );
    if !p_bool(params, "include_text", true) {
        if let Some(obj) = v.as_object_mut() {
            obj.remove("text");
        }
    }
    if p_bool(params, "include_outer_html", false) {
        v["outer_html"] = json!(document::outer_html(node));
    }
    // 可见性以计算样式为准(外链/内联 CSS 的 display/visibility 都生效),
    // 且必须与布局层剪枝规则一致:任一祖先 `display:none` 时整棵子树不参与
    // 布局,此时元素没有几何 —— 不能自报"可见"却给不出矩形。
    let engine = style_engine(tab, doc);
    let computed = engine.computed_chain(node);
    let rendered = engine.is_rendered(node);
    let visible = rendered
        && computed.display != "none"
        && computed.visibility != "hidden"
        && computed.visibility != "collapse"
        && computed.opacity > 0.0;
    v["visible"] = json!(visible);
    if let Some(rect) = v.get_mut("rect").and_then(Value::as_object_mut) {
        rect.insert("visible".to_string(), json!(visible));
        // `laid_out=false` 表示该元素没有布局盒(被剪枝或非可寻址行内元素),
        // 与"有盒子但尺寸为 0"区分开,避免 Agent 误判。
        rect.insert("laid_out".to_string(), json!(rect_found));
    }
    if !rendered {
        v["render_blocked_by"] = json!("ancestor_display_none");
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
    let mut fields = params
        .get("fields")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    // 未声明 fields 时自动兜底:否则每条 item 都会是空对象 `{}`,
    // Agent 拿到一堆无内容的记录却不知道缺了字段(实测踩坑)。
    let auto_fields = fields.is_empty();
    if auto_fields {
        fields.insert("text".to_string(), json!({"selector": ""}));
    }
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
        "auto_fields": auto_fields,
        "hostname": super::urlutil::host_of(&doc.base_url),
        "hint": "阶段一按 CSS 选择器抽取;页面脚本渲染内容需阶段二支持。未声明 fields 时自动抽取 text 字段",
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
        // 阻断词必须带访问/拦截语义:招聘、安全资讯等正常页面会出现
        // 「风控算法」「风控工程师」,不能把业务名词当成站点风控页。
        ("risk", &["异常访问", "访问异常", "风险拦截", "访问被拦截", "risk control"]),
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
