//! MCP_Web_Use action=inspect:全部只读观察统一入口(自 tools/browser.rs 平移)。
//!
//! `info` 枚举(19 个)覆盖 Console / Network / Elements / DOM / localStorage /
//! sessionStorage / Cookies / 截图 / 页面元信息 / 视口 / URL / 标题 / ping /
//! 图片 URL / OCR / 人工阻断检测 / 链接批量提取 / 结构化列表抽取(`extract`,
//! 见 `extract.rs`) / 页面 SSR 注水数据直读(`page_state`,见 `page_state.rs`)。

use serde_json::{json, Value};

use super::*;
use super::control::act_screenshot;

/// 阻断模式表:(kind, 关键词列表, 处置建议)。
const BLOCKER_PATTERNS: &[(&str, &[&str], &str)] = &[
    (
        "captcha",
        &[
            "captcha", "验证码", "滑动验证", "拖动滑块", "拼图验证", "recaptcha",
            "人机验证", "安全验证",
        ],
        "图形/滑块验证码:OCR 可读走 screenshot(ocr=true) 自动链;滑块/不可读用 control(request_human, reason=captcha)",
    ),
    (
        "sms",
        &["短信验证码", "手机验证码", "验证码已发送", "sms code", "短信校验码"],
        "短信验证码:Agent 无法获取手机短信,control(request_human, reason=sms) 让人工在 TUI 直接输入数字",
    ),
    (
        "qr_login",
        &["扫码登录", "二维码登录", "扫描二维码", "qr code", "微信扫码", "支付宝扫码"],
        "扫码登录:人工用手机扫码,control(request_human, reason=qr_login)",
    ),
    (
        "login",
        &["请登录", "登录后查看", "立即登录", "sign in", "log in to continue", "账号登录"],
        "登录墙:用户已提供凭证则自动登录;否则 control(request_human, reason=login) 让人工在窗口登录",
    ),
    (
        "real_name",
        &[
            "实名认证", "实名验证", "身份认证", "身份验证", "人脸核身", "人脸识别",
            "人脸验证", "上传身份证", "证件认证", "kyc", "real-name", "real name",
        ],
        "实名认证/人脸核身/上传身份证:Agent 无法代为核验,control(request_human, reason=real_name) 让用户在浏览器窗口完成验证后继续",
    ),
    (
        "two_factor",
        &[
            "两步验证", "两步校验", "双重验证", "二次验证", "动态口令", "安全令牌",
            "google authenticator", "2fa", "2-step", "邮箱验证码", "邮件验证码",
            "邮箱校验码", "动态密码", "totp",
        ],
        "二次验证/2FA/TOTP/邮箱验证码:用户从手机/邮箱获取动态码,control(request_human, reason=two_factor) 让人工在 TUI 直接输入数字",
    ),
    (
        "oauth",
        &[
            // 中国常见 OAuth 入口
            "微信授权", "支付宝授权", "github 授权", "google 授权",
            "github 账号", "google 账号", "微信 账号", "支付宝 账号",
            // 「使用 X」/「continue with X」 引导语
            "使用 github", "使用 google", "使用 微信", "使用 支付宝", "使用 qq",
            "continue with", "sign in with",
            // 授权/SSO 通用词
            "授权登录", "第三方登录", "sso 登录", "single sign-on",
            "oauth", "open in app", "应用授权",
        ],
        "第三方授权/SSO/OAuth:Agent 无法跨设备授权,control(request_human, reason=oauth) 让用户在浏览器窗口完成授权后继续",
    ),
];

/// 在页面文本中检测人工阻断(Rust 侧模式表,可单测)。
/// 返回命中列表 [{kind, snippet}](每种 kind 最多 1 条,避免重复刷屏)。
pub(super) fn detect_blockers(text: &str) -> Vec<Value> {
    let lower = text.to_lowercase();
    let mut out: Vec<Value> = Vec::new();
    for (kind, keywords, _) in BLOCKER_PATTERNS {
        if let Some(kw) = keywords
            .iter()
            .find(|kw| lower.contains(&kw.to_lowercase()))
        {
            // 截取关键词上下文片段(前 12 后 40 字符),便于 LLM 确认
            if let Some(pos) = lower.find(&kw.to_lowercase()) {
                let start = pos.saturating_sub(12).min(text.len());
                // 对齐 char 边界,防中文截断 panic
                let start = text
                    .char_indices()
                    .map(|(i, _)| i)
                    .find(|i| *i >= start)
                    .unwrap_or(start);
                let end = (pos + kw.len() + 40).min(text.len());
                let end = text
                    .char_indices()
                    .rev()
                    .map(|(i, _)| i)
                    .find(|i| *i <= end)
                    .unwrap_or(end);
                let snippet: String = text
                    .get(start..end)
                    .map(|s| s.replace(['\n', '\r', '\t'], " "))
                    .unwrap_or_default();
                out.push(json!({
                    "kind": kind,
                    "matched": kw,
                    "snippet": snippet,
                }));
            }
        }
    }
    out
}

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
    // 第 146 轮:覆盖台账查询是**进程级全局事实**,不依赖任何存活页面 ——
    // 提前于 page_id 解析分发(全部页面已 close 时也能对账)。
    if kind == "coverage" {
        return envelope(0, "ok", super::visit_ledger::coverage_payload());
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
            // 第 103 轮:selector 可选,缺失时默认返回全页交互元素列表
            // (input/button/select/textarea/a/[role=button]/[contenteditable=true]),
            // 消除 LLM 因 Schema additionalProperties:false 无法在顶层传入 selector
            // 而反复试错浪费迭代的低频错误。
            let sel = str_arg(&params, "selector")
                .unwrap_or("input,button,select,textarea,a,[role=button],[role=link],[contenteditable=true]");
            let nth = params.get("nth").and_then(Value::as_u64).unwrap_or(0) as usize;
            let include_text = params.get("include_text").and_then(Value::as_bool).unwrap_or(true);
            let include_outer = params.get("include_outer_html").and_then(Value::as_bool).unwrap_or(false);
            let js = format!(
                // 过滤 [data-laew-agent] 子树:Agent 高亮蓝框 overlay 不参与元素观察
                r#"(() => {{ const els=[...document.querySelectorAll({sel})].filter(e=>!e.closest('[data-laew-agent]')); const el=els[{nth}];
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
                    // 过滤 Agent 高亮 overlay(#[data-laew-agent] 不属于页面真实 DOM)
                    r#"(() => {{ const el=[...document.querySelectorAll({sel})].find(e=>!e.closest('[data-laew-agent]'));
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
                            // Agent 高亮 overlay 子树不进入节点树(真实页面结构观察)
                            if (n.nodeType===1 && n.closest && n.closest('[data-laew-agent]')) return null;
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
        // 第 99 轮:截图 + OCR 一体 —— 验证码/图表标签等图片文字的一步读取
        // (macOS Vision;region 过滤词块;图像默认落临时目录,响应带 save_path)。
        "ocr" => {
            let mut p = params.clone();
            p["ocr"] = json!(true);
            act_screenshot(id, &p).await
        }
        // 第 100 轮:人工阻断检测 —— 验证码/短信/扫码/登录墙的确定性启发式,
        // 作为 SubAgent「无法自动跳过 → request_human」的判定依据。
        "blockers" => {
            let text = eval_js_string(
                &page,
                // 克隆 body 并剥离 [data-laew-agent] 子树再读 innerText:
                // Agent 高亮徽标文本(「LAEW Agent 控制中」)不应参与阻断判定。
                r#"(() => { try {
                    if (!document.body) return '';
                    const clone = document.body.cloneNode(true);
                    clone.querySelectorAll('[data-laew-agent]').forEach(e => e.remove());
                    return clone.innerText.slice(0, 20000);
                } catch(e) { return ''; } })()"#,
            )
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
            let blockers = detect_blockers(&text);
            // 暴露合法 reason 列表:LLM 拿到 blockers 后可直接挑一个去 request_human,
            // 不必反查工具 schema / 文档。新增 reason 时本列表自动跟随常量更新。
            let allowed_reasons: Vec<&'static str> =
                super::control::HUMAN_ASSIST_ALLOWED_REASONS.to_vec();
            let mut out = json!({
                "blockers": blockers,
                "blocked": !blockers.is_empty(),
                "suggested_action": if blockers.is_empty() { "continue" } else { "request_human" },
                "available_reasons": allowed_reasons,
            });
            // 第 131 轮:命中阻断时,把「下一步=request_human」的完整载荷(推荐 reason /
            // message / options)一并挂上 —— 原先只有 `suggested_action` 一个裸字符串,
            // 模型仍要自己去猜 reason 与提示文案。
            if out["blocked"].as_bool() == Some(true) {
                let kind = blockers
                    .first()
                    .and_then(|b| b.get("kind"))
                    .and_then(Value::as_str)
                    .filter(|k| super::control::HUMAN_ASSIST_ALLOWED_REASONS.contains(k))
                    .unwrap_or("manual_verify")
                    .to_string();
                let hint = super::human_assist_hint(
                    &kind,
                    "页面存在人工阻断(见 data.blockers),请人工完成该步骤后继续",
                    &["我已完成人工操作,继续", "取消任务"],
                );
                super::merge_hint(&mut out, hint);
                // 第 144 轮:顺带探测凭证输入区(账号/密码/验证码),把选择器挂进
                // 响应与人机载荷 —— LLM 可直接把 allow_selectors 传给 request_human
                // 精准放行(自动档探测不命中时的人工兜底通道);fail-open,探测为空
                // 不加字段(request_human 自动档还有一次机会)。
                let zones = super::unlock_zone::probe_credential_zones(&page).await;
                if !zones.is_empty() {
                    out["credential_zones"] = json!(zones.clone());
                    if let Some(ha) = out.get_mut("human_assist").and_then(Value::as_object_mut) {
                        ha.insert("allow_selectors".into(), json!(zones));
                    }
                }
            }
            Ok(out)
        }
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
        // 第 125 轮:视口观察增强 —— 内容尺寸取 documentElement/body 双源最大值,
        // 附 overflow 溢出判定与 next_action 对策(横向裁切=显示不全根因)。
        "viewport" => match eval_js_string(&page,
            "({width: window.innerWidth, height: window.innerHeight, devicePixelRatio: window.devicePixelRatio, scrollX: window.scrollX, scrollY: window.scrollY, scrollWidth: Math.max(document.documentElement.scrollWidth, document.body ? document.body.scrollWidth : 0), scrollHeight: Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0)})"
        ).await {
            Err(e) => Err(e),
            Ok(mut v) => {
                if let (Some(w), Some(h), Some(sw), Some(sh)) = (
                    v.get("width").and_then(Value::as_f64),
                    v.get("height").and_then(Value::as_f64),
                    v.get("scrollWidth").and_then(Value::as_f64),
                    v.get("scrollHeight").and_then(Value::as_f64),
                ) {
                    let horizontal = sw > w + 2.0;
                    let vertical = sh > h + 2.0;
                    v["overflow"] = json!({
                        "horizontal": horizontal,
                        "vertical": vertical,
                        "content_larger_than_viewport": horizontal || vertical,
                    });
                    v["next_action"] = if horizontal {
                        json!("内容横向被裁(页面显示不全/元素不可点的根因):重开 open(默认自动扩展视口到 2K)或 control(set_viewport, width=scrollWidth 对应值);仅截图看全内容用 screenshot params.full_page=true")
                    } else if vertical {
                        json!("纵向可滚动属正常:整页截图用 params.full_page=true;长文提取优先 inspect(elements/dom) 而非截图")
                    } else {
                        json!("continue")
                    };
                }
                Ok(v)
            }
        },
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
        // 第 N 轮:链接批量提取 —— 一键提取页面所有链接(href + 文本 + 上下文),
        // 用于「自动遍历子页面、找文章/新闻列表、按时间排序」等场景。
        // 替代此前 LLM 需要多次 inspect(elements, nth=0/1/2...) 逐条提取的低效方式。
        "extract_links" => {
            let sel = str_arg(&params, "selector").unwrap_or("a");
            let max_n = params.get("max_links").and_then(Value::as_u64).unwrap_or(200).min(500) as usize;
            let include_context = params.get("include_context").and_then(Value::as_bool).unwrap_or(true);
            let ctx_len = params.get("context_length").and_then(Value::as_u64).unwrap_or(200).min(500) as usize;
            let host = eval_js_string(&page, "window.location.hostname")
                .await
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let js = format!(
                r#"(() => {{
                    const SEL = {sel}, MAX = {max_n}, CTX = {ctx_len}, INC = {inc};
                    const HOST = {host};
                    const links = [];
                    const seen = new Set();
                    const nodes = document.querySelectorAll(SEL);
                    for (const a of nodes) {{
                        if (links.length >= MAX) break;
                        if (a.closest && a.closest('[data-laew-agent]')) continue;
                        const href = (a.href || '').trim();
                        if (!href || href.startsWith('javascript:') || href.startsWith('#')) continue;
                        const text = ((a.innerText || a.textContent || '').replace(/\s+/g, ' ').trim()).slice(0, 200);
                        if (!text) continue;
                        const key = href + '||' + text;
                        if (seen.has(key)) continue;
                        seen.add(key);
                        let context = '';
                        if (INC) {{
                            const container = a.closest('article, .post, .news, .blog, section, li, div[class*="article"], div[class*="post"], div[class*="news"], div[class*="item"]');
                            if (container) {{
                                context = (container.innerText || '').replace(/\s+/g, ' ').trim().slice(0, CTX);
                            }}
                        }}
                        links.push({{
                            href: href,
                            text: text,
                            context: context,
                            is_external: !href.includes(HOST)
                        }});
                    }}
                    return {{
                        links: links,
                        total: links.length,
                        truncated: nodes.length > MAX,
                        scanned: nodes.length,
                        hostname: HOST
                    }};
                }})()"#,
                sel = js_str(sel),
                max_n = max_n,
                ctx_len = ctx_len,
                inc = include_context,
                host = js_str(&host),
            );
            eval_js_string(&page, &js).await
        }
        // 第 135 轮:结构化列表 / 表格抽取(见 extract.rs)—— 替代「手写 eval_js +
        // 猜字段层级 + 大对象 dump」的三连坑;probe=true 时只做选择器侦查。
        "extract" => extract::run(&page, &params).await,
        // 第 135 轮:页面内 SSR 注水数据直读(见 page_state.rs)—— 让模型不必
        // 猜 `window.__NEXT_DATA__` 之类的全局变量名。
        "page_state" => page_state::run(&page, &params).await,
        other => return envelope(1001, "未知 info", json!({"info": other})),
    };
    match res {
        Ok(data) => envelope(0, "ok", gate_payload(data)),
        Err(e) => envelope(2002, &e, json!({})),
    }
}

/// inspect 返回体体积闸门(第 140 轮提速优化)。
///
/// `eval_sanitize` 的 20KB 闸门只作用于 `control(eval_js)`,inspect 各维度
/// (`dom(selector)` 的 outerHTML / storage / cookies 等)在此前直接把超大返回体
/// 灌进 LLM 上下文 —— 实测任务 avg_in=39K/次的输入膨胀来源之一,拖慢每一次
/// 后续 LLM 往返。两段闸门:先逐字符串裁剪,整体仍超限则降级为摘要。
const INSPECT_BYTE_LIMIT: usize = 24 * 1024;
const INSPECT_STR_LIMIT: usize = 8 * 1024;

pub(super) fn gate_payload(v: Value) -> Value {
    let mut v = v;
    cap_strings(&mut v, INSPECT_STR_LIMIT);
    let bytes = serde_json::to_vec(&v).map(|b| b.len()).unwrap_or(0);
    if bytes <= INSPECT_BYTE_LIMIT {
        return v;
    }
    // 整体降级:保留可辨识摘要,显式告知被裁(模型可换 selector / extract 重取)
    let preview = crate::logging::clip_for_log(&v.to_string(), 2000);
    json!({
        "truncated": true,
        "original_bytes": bytes,
        "hint": "inspect 返回体超过 24KB 已裁剪;请缩小 selector 范围 / 用 max_depth、limit 分段,或改用 inspect(info=extract) 结构化抽取",
        "preview": preview,
    })
}

/// 递归裁剪超长字符串(标记原长,保留头 8K)。
pub(super) fn cap_strings(v: &mut Value, max_chars: usize) {
    match v {
        Value::String(s) => {
            if s.chars().count() > max_chars {
                let total = s.chars().count();
                let head: String = s.chars().take(max_chars).collect();
                *s = format!("{head}…(截断,共 {total} 字符)");
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(|x| cap_strings(x, max_chars)),
        Value::Object(map) => map.values_mut().for_each(|x| cap_strings(x, max_chars)),
        _ => {}
    }
}
