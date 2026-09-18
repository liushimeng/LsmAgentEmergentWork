//! MCP_Web_Use action=inspect:全部只读观察统一入口(自 tools/browser.rs 平移)。
//!
//! `info` 枚举(14 个)覆盖 Console / Network / Elements / DOM / localStorage /
//! sessionStorage / Cookies / 截图 / 页面元信息 / 视口 / URL / 标题 / ping / 图片 URL。

use serde_json::{json, Value};

use super::*;
use super::control::act_screenshot;

/// inspect 分发入口。
pub(super) async fn run(args: Value) -> crate::error::Result<String> {
    let Some(id) = str_arg(&args, "page_id") else {
        return envelope(1001, "缺少 page_id", json!({}));
    };
    let Some(kind) = str_arg(&args, "info") else {
        return envelope(1001, "缺少 info", json!({}));
    };
    let params = args.get("params").cloned().unwrap_or_else(|| json!({}));
    if kind == "ping" {
        let ms = now_millis_safe();
        return envelope(0, "ok", json!({"ok": true, "page_id": id, "latency_ms": 0, "ts": ms}));
    }
    let page = match crate::agent::browser::BrowserManager::global().page(id).await {
        Some(p) => p,
        None => return envelope(2000, "page_id 不存在", json!({"page_id": id})),
    };
    let res: std::result::Result<Value, String> = match kind {
        "console" => {
            let buf = crate::agent::browser::BrowserManager::global().events(id).await;
            let (healthy, last) = match &buf {
                Some(b) => b.collection_healthy(),
                None => (true, None),
            };
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(100).min(500);
            let events: Vec<Value> = buf.map(|b| {
                let q = b.console.lock().unwrap();
                q.iter().rev().take(limit as usize).rev().map(|e| {
                    let mut v = e.data.clone();
                    v["ts"] = json!(e.ts_ms);
                    v
                }).collect()
            }).unwrap_or_default();
            Ok(json!({"events": events, "count": events.len(), "collection_healthy": healthy, "last_event_at": last}))
        }
        "network" => {
            let buf = crate::agent::browser::BrowserManager::global().events(id).await;
            let (healthy, last) = match &buf {
                Some(b) => b.collection_healthy(),
                None => (true, None),
            };
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(100).min(500);
            let events: Vec<Value> = buf.map(|b| {
                let q = b.network.lock().unwrap();
                q.iter().rev().take(limit as usize).rev().map(|e| {
                    let mut v = e.data.clone();
                    v["ts"] = json!(e.ts_ms);
                    v
                }).collect()
            }).unwrap_or_default();
            Ok(json!({"events": events, "count": events.len(), "collection_healthy": healthy, "last_event_at": last}))
        }
        "elements" => {
            let Some(sel) = str_arg(&params, "selector") else {
                return envelope(1001, "缺 selector", json!({}));
            };
            let nth = params.get("nth").and_then(Value::as_u64).unwrap_or(0) as usize;
            let include_text = params.get("include_text").and_then(Value::as_bool).unwrap_or(true);
            let include_outer = params.get("include_outer_html").and_then(Value::as_bool).unwrap_or(false);
            let js = format!(
                r#"(() => {{ const els=document.querySelectorAll({sel}); const el=els[{nth}];
                if(!el) return {{ok:false, count: els.length, index:{nth}}};
                const r=el.getBoundingClientRect();
                return {{ok:true, tag:el.tagName.toLowerCase(), count:els.length, index:{nth},
                    text: ({itxt} ? el.innerText : null),
                    outer_html: ({iouter} ? el.outerHTML : null),
                    rect: {{x:r.x,y:r.y,width:r.width,height:r.height,visible:r.width>0&&r.height>0}}
                }}; }})()"#,
                sel = js_str(sel), nth = nth, itxt = include_text, iouter = include_outer
            );
            eval_js_string(&page, &js).await
        }
        "dom" => {
            let sel = str_arg(&params, "selector").unwrap_or("html");
            if sel != "html" {
                let js = format!(
                    r#"(() => {{ const el=document.querySelector({sel});
                    if(!el) return {{ok:false, selector:{sel}}};
                    return {{ok:true, selector:{sel}, outer_html:el.outerHTML}}; }})()"#,
                    sel = js_str(sel)
                );
                eval_js_string(&page, &js).await
            } else {
                let max_depth = params.get("max_depth").and_then(Value::as_i64).unwrap_or(3).clamp(1, 10);
                let node_limit = params.get("node_count_limit").and_then(Value::as_u64).unwrap_or(2000).min(5000);
                let js = format!(
                    r#"(() => {{
                        const MAX_DEPTH={max_depth}, NODE_LIMIT={node_limit};
                        let nodes=0, truncated=false;
                        function walk(n, d) {{
                            if (nodes>=NODE_LIMIT) {{ truncated=true; return null; }}
                            if (d>MAX_DEPTH) return null;
                            const obj={{
                                node_type: n.nodeType, node_name: n.nodeName,
                                node_value: n.nodeValue, attributes: {{}};
                            }};
                            nodes++;
                            if (n.nodeType===1) {{
                                for (const a of n.attributes) obj.attributes[a.name]=a.value;
                                const children=[];
                                for (const c of n.childNodes) {{
                                    const r=walk(c, d+1); if (r) children.push(r);
                                    if (truncated) break;
                                }}
                                if (children.length) obj.children=children;
                            }}
                            return obj;
                        }}
                        const tree=walk(document, 0);
                        return {{document: tree, node_count:nodes, truncated, max_depth:MAX_DEPTH}};
                    }})()"#,
                    max_depth = max_depth, node_limit = node_limit
                );
                eval_js_string(&page, &js).await
            }
        }
        "localstorage" | "sessionstorage" => {
            let storage_obj = if kind == "sessionstorage" { "sessionStorage" } else { "localStorage" };
            let prefix = str_arg(&params, "prefix");
            let contains = str_arg(&params, "contains");
            let filter_js = match (prefix, contains) {
                (Some(p), None) => format!("(k.startsWith({}))", js_str(p)),
                (None, Some(c)) => format!("(k.includes({}))", js_str(c)),
                (Some(p), Some(c)) => format!("(k.startsWith({}) && k.includes({}))", js_str(p), js_str(c)),
                (None, None) => "(() => true)".to_string(),
            };
            let js = format!(
                r#"(() => {{ const s={storage}; const out={{}};
                    for (let i=0;i<s.length;i++) {{ const k=s.key(i); if ({filter}) out[k]=s.getItem(k); }}
                    return out; }})()"#,
                storage = storage_obj, filter = filter_js
            );
            eval_js_string(&page, &js).await
        }
        "cookies" => {
            let r = match page.execute(
                chromiumoxide::cdp::browser_protocol::network::GetCookiesParams::default(),
            ).await {
                Ok(r) => r,
                Err(e) => return envelope(2002, &e.to_string(), json!({})),
            };
            let cookies: Vec<Value> = r.cookies.iter().map(|c| json!({
                "name": c.name, "value": c.value, "domain": c.domain, "path": c.path,
                "expires": c.expires, "http_only": c.http_only, "secure": c.secure,
                "same_site": format!("{:?}", c.same_site),
            })).collect();
            Ok(json!({"cookies": cookies}))
        }
        "screenshot" => act_screenshot(id, &params).await,
        "page_meta" => {
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            let viewport = match eval_js_string(&page, "({w: window.innerWidth, h: window.innerHeight})").await {
                Ok(v) => v,
                Err(e) => json!({"error": e}),
            };
            let ua = page
                .evaluate("navigator.userAgent")
                .await
                .ok()
                .and_then(|v| v.value().cloned())
                .unwrap_or(Value::Null);
            Ok(json!({"page_id": id, "url": url, "title": title, "viewport": viewport, "user_agent": ua}))
        }
        "viewport" => eval_js_string(&page,
            "({width: window.innerWidth, height: window.innerHeight, devicePixelRatio: window.devicePixelRatio, scrollX: window.scrollX, scrollY: window.scrollY, scrollWidth: document.documentElement.scrollWidth, scrollHeight: document.documentElement.scrollHeight})"
        ).await,
        "url" => Ok(json!({"url": page.url().await.ok().flatten().unwrap_or_default()})),
        "title" => Ok(json!({"title": page.get_title().await.ok().flatten().unwrap_or_default()})),
        // image_urls(第 64 轮):一键提取页面图片 URL + canvas/svg 计数。
        // 文心一言 K 线图 / ChatGPT 图表 / Claude.ai 生成的图都是 <img src=...> 或 canvas。
        // 限制 img_urls 最多 20 条避免大页面输出爆炸;canvas/svg 只计数。
        "image_urls" => {
            let max_n = params.get("max").and_then(Value::as_u64).unwrap_or(20).min(100) as usize;
            let js = format!(
                r#"(() => {{
                    const N = {max_n};
                    const imgs = Array.from(document.querySelectorAll('img'))
                        .map(i => i.src || i.getAttribute('data-src') || '')
                        .filter(Boolean);
                    const data_imgs = imgs.filter(s => s.startsWith('data:image/'));
                    const canvases = document.querySelectorAll('canvas').length;
                    const svgs = document.querySelectorAll('svg').length;
                    const pics = document.querySelectorAll('picture').length;
                    return {{
                        img_count: imgs.length,
                        img_urls: imgs.slice(0, N),
                        data_image_count: data_imgs.length,
                        canvas_count: canvases,
                        svg_count: svgs,
                        picture_count: pics
                    }};
                }})()"#,
                max_n = max_n,
            );
            eval_js_string(&page, &js).await
        }
        other => return envelope(1001, "未知 info", json!({"info": other})),
    };
    match res {
        Ok(data) => envelope(0, "ok", data),
        Err(e) => envelope(2002, &e, json!({})),
    }
}
