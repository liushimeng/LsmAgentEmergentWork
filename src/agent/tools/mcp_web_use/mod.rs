//! MCP_Web_Use 工具(2026-09-18 第 89 轮):浏览器网页操控的统一 MCP 风格入口。
//!
//! 由原 Chromium-WebUse Agent(第 11 角色)的 5 个独立工具(BrowserNew / BrowserList /
//! BrowserClose / BrowserControl / BrowserInspect)收敛而来:单工具 + `action` 枚举
//! 分发,结构化 JSON 入参/出参,由持有该工具的 Agent(SubAgent-Work)在自身
//! 多轮对话循环中反复调用。
//!
//! - CDP 协议与浏览器进程管理封闭在 `agent::browser` 驱动层(本工具的"MCP 服务"实现,
//!   chromiumoxide,Chrome / Edge / Chromium / Brave,Windows / macOS / Linux),
//!   本模块只做参数校验、action 分发与输出组织;
//! - `page_id` 全生命周期:open 拿 id → control/inspect 多轮复用 → 点击链接/新开标签页
//!   经 `spawned_page_id` 回传新 id → close 释放(最后一个页面关闭时回收浏览器进程);
//! - 所有 action 返回统一 JSON 信封 `{code,message,data}`(0 成功,1001 参数错误,
//!   2000 page_id 失效,2001 断连,2002 动作失败,2003 页面崩溃,3001 未检测到浏览器)
//!   —— Agent 据错误码做机械决策(2000 → action=list 重新同步;2002 → 换 selector/路径重试);
//! - **无平台门控**:CDP 三平台行为一致,未安装浏览器时返回结构化 3001 信封 + 安装引导
//!   (不崩溃),因此全平台注册进 `builtin_registry()` 并同步注入系统提示词使用说明。
//!
//! 设计见 `docs/MCP_Web_Use/01-设计与解决方案.md`(唯一最新版)。
//! 技术参考:`docs/浏览器CDP工具/Rust操作Chrome浏览器CDP完整技术方案.md`。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Tool;
use crate::agent::browser::{BrowserManager, NO_BROWSER_SENTINEL};
use crate::error::Result;

mod control;
mod inspect;

#[cfg(test)]
mod tests;

/// 工具名(LLM 可见的唯一浏览器操控入口)。
pub const MCP_WEB_USE_TOOL_NAME: &str = "MCP_Web_Use";

// ===================== 共享辅助(自 tools/browser.rs 平移) =====================

pub(super) fn envelope(code: i32, message: &str, data: Value) -> Result<String> {
    Ok(json!({"code": code, "message": message, "data": data}).to_string())
}

pub(super) fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// 把字符串安全嵌入 JS 字面量(转义 `\`, `'`, 控制字符)。
pub(super) fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

pub(super) fn now_millis_safe() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

/// 调 JS 并以 returnByValue 提取结果。
pub(super) async fn eval_js_string(
    page: &chromiumoxide::Page,
    js: &str,
) -> std::result::Result<Value, String> {
    use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
    let params = EvaluateParams::builder()
        .expression(js.to_string())
        .return_by_value(true)
        .build()
        .map_err(|e| e.to_string())?;
    let v = page.evaluate(params).await.map_err(|e| e.to_string())?;
    Ok(v.value().cloned().unwrap_or(Value::Null))
}

/// 找元素中心点 (x,y,w,h,visible);不存在返回 Err("not_found")。
pub(super) async fn eval_find_center(
    page: &chromiumoxide::Page,
    selector: &str,
    nth: usize,
) -> std::result::Result<Value, String> {
    let sel = js_str(selector);
    let js = format!(
        r#"(() => {{
            const els = document.querySelectorAll({sel});
            const el = els[{nth}];
            if (!el) return null;
            try {{ el.scrollIntoView({{block:'center'}}); }} catch(e) {{}}
            const r = el.getBoundingClientRect();
            return {{x: r.x + r.width/2, y: r.y + r.height/2, w: r.width, h: r.height,
                    visible: r.width>0 && r.height>0}};
        }})()"#,
    );
    eval_js_string(page, &js).await
        .and_then(|v| if v.is_null() { Err("not_found".into()) } else { Ok(v) })
}

pub(super) async fn ensure_page(id: &str) -> std::result::Result<chromiumoxide::Page, String> {
    BrowserManager::global().page(id).await.ok_or_else(|| "page_id 不存在".into())
}

/// 从工具输出文本中提取 page_id(2026-09-16 第 64 轮起持续使用)。
///
/// 仅匹配 action=open 成功时返回的 `data.page_id:"p_xxxxxxxx"`(8 位 hex),
/// 防止误匹配其它字段。失败(code=3001 等)时无 page_id,返回 None。
pub(crate) fn extract_page_id_from_text(text: &str) -> Option<String> {
    const PREFIX: &str = "\"page_id\":\"p_";
    let start = text.find(PREFIX)?;
    let after = &text[start + PREFIX.len()..];
    let end = after
        .find('"')
        .map(|i| start + PREFIX.len() + i + 1)
        .unwrap_or(text.len());
    let pid_start = start + PREFIX.len() - 2; // 含 "p_"
    Some(text[pid_start..end].trim_matches('"').to_string())
}

// ===================== 第 99 轮:open 复用 / close 清场 辅助 =====================

/// URL 归一化(复用匹配用):去尾部 `/` 与 `#fragment` 后精确比较。
pub(super) fn normalize_web_url(u: &str) -> String {
    let mut s = u.trim().to_string();
    if let Some(i) = s.find('#') {
        s.truncate(i);
    }
    while s.ends_with('/') {
        s.pop();
    }
    s
}

/// 在已存活页面中找同 URL 页面(第 99 轮:open 复用,根治重试/重复 open
/// 导致的页面泄漏)。基于 `list_pages()` 公共 API(自带失效 entry 清理)。
async fn find_reusable_page_id(url: &str) -> Option<String> {
    let want = normalize_web_url(url);
    BrowserManager::global()
        .list_pages()
        .await
        .into_iter()
        .find(|(_, u, _, _)| normalize_web_url(u) == want)
        .map(|(id, _, _, _)| id)
}

/// open 成功响应的四步引导(新建与复用两条路径共用)。
fn open_next_steps() -> Value {
    json!([
        {"step": 1, "action": "control", "control_action": "input_text",
         "selector_hint": "textarea, [contenteditable=true], input[type=text]",
         "tip": "在对话框/输入框输入你的查询文本"},
        {"step": 2, "action": "control", "control_action": "click",
         "selector_hint": "button[type=submit], .submit-btn, [class*=send], [class*=submit], img[class*=button]",
         "tip": "点击提交按钮(图片按钮可用 selector 命中 img 元素)"},
        {"step": 3, "action": "control", "control_action": "wait",
         "selector_hint": "[class*=response], [class*=answer], [class*=result], [class*=message]",
         // AI 类网站长尾响应常见 30-60s(第 82 轮)
         "timeout_ms": 60000,
         "tip": "等待 AI 回复出现,最长等 60 秒"},
        {"step": 4, "action": "inspect", "info": "elements",
         "selector_hint": "[class*=response], [class*=answer], [class*=result]",
         "include_text": true,
         "tip": "提取 AI 回复文本;若 include_text 太短,可改 info=dom 获取 outer_html"}
    ])
}

/// 解析 open 的 `window_width`/`window_height`(第 100 轮):
/// 任一缺失返回 None(由驱动层按 mode 给默认);均存在时 clamp 到安全区间
/// (320~7680 / 240~4320,覆盖 1080p/2K/4K 与最小可读尺寸)。
fn parse_window_size(args: &Value) -> Option<(u32, u32)> {
    let w = args.get("window_width").and_then(Value::as_u64)?;
    let h = args.get("window_height").and_then(Value::as_u64)?;
    let w = w.clamp(320, 7680) as u32;
    let h = h.clamp(240, 4320) as u32;
    Some((w, h))
}

// ===================== action=open(list/close 同级,轻量内联) =====================

/// 启动/接管浏览器并打开一个页面(原 BrowserNew)。
///
/// 返回 `{page_id,title,final_url,mode,next_steps}`;`next_steps` 是 4 步
/// input_text/click/wait/elements 引导(带 selector_hint),Agent 循环在
/// open 成功后会把 next_steps 注入 LLM 上下文(见 `agent_loop.rs` 引导钩子)。
///
/// 第 99 轮:同 URL 复用 —— 已存在同 URL 存活页面时默认导航刷新并返回同一
/// page_id(`reused:true`),根治「重试/重复 open 泄漏页面」;`reuse:false`
/// 强制新开。
///
/// 第 100 轮:窗口可视化 —— `window_width/window_height`(headed 默认 1920×1080
/// 1080p)、`highlight`(默认 true,蓝色选中边框标识 Agent 控制窗口);浏览器实例
/// 已存在时永远复用同一进程(响应 `browser_reused:true` + 真实 mode),不重复开浏览器。
async fn run_open(args: Value) -> Result<String> {
    let Some(url) = str_arg(&args, "url") else {
        return envelope(1001, "缺少 url", json!({}));
    };
    let reuse = args.get("reuse").and_then(Value::as_bool).unwrap_or(true);
    // 第 103 轮:读取 timeout_ms 参数,控制页面加载超时(默认 60s)
    let timeout_ms = args.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(60000);
    if reuse {
        if let Some(pid) = find_reusable_page_id(url).await {
            // 复用路径:在同一页面上导航刷新(新验证码/新会话态),page_id 不变;
            // 导航失败(页面挂死/断连)时落回新建路径。
            if let Some(page) = BrowserManager::global().page(&pid).await {
                let goto = tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    page.goto(url.to_string()),
                )
                .await;
                if let Ok(Ok(_)) = goto {
                    // 第 100 轮:复用路径同样带单实例元数据(浏览器进程级复用 + 真实 mode)
                    let (browser_reused, mode_label) = {
                        let mgr = BrowserManager::global();
                        let m = mgr.current_mode().await;
                        (
                            mgr.has_browser().await,
                            match m.unwrap_or(crate::agent::browser::BrowserMode::Hidden) {
                                crate::agent::browser::BrowserMode::Hidden => "hidden",
                                crate::agent::browser::BrowserMode::NewHeadless => "new_headless",
                                crate::agent::browser::BrowserMode::Headed => "headed",
                            },
                        )
                    };
                    let title = page.get_title().await.ok().flatten().unwrap_or_default();
                    let final_url = page
                        .url()
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| url.to_string());
                    return envelope(
                        0,
                        "ok",
                        json!({
                            "page_id": pid,
                            "title": title,
                            "final_url": final_url,
                            "reused": true,
                            "browser_reused": browser_reused,
                            "mode": mode_label,
                            "next_steps": open_next_steps(),
                        }),
                    );
                }
            }
        }
    }
    // 三档 mode(第 74 轮):默认 hidden(纯 CDP 无窗口),解决"误开系统默认浏览器"。
    let requested_mode = match str_arg(&args, "mode") {
        Some("headed") | Some("head") => crate::agent::browser::BrowserMode::Headed,
        Some("new_headless") | Some("old_headless") => crate::agent::browser::BrowserMode::NewHeadless,
        _ => crate::agent::browser::BrowserMode::Hidden,
    };
    // 单浏览器实例语义(第 100 轮):实例已存在时永远复用,绝不因 mode/参数差异
    // 新启第二个浏览器进程;请求 mode 与实例不一致时回真实 mode + mode_hint。
    let (browser_reused, effective_mode, mode_hint) =
        match BrowserManager::global().current_mode().await {
            Some(existing) => {
                let hint = if existing != requested_mode {
                    Some(format!(
                        "已复用现有浏览器实例(实际 mode={});如需 {} 可视化,先 close(page_id=\"all\") 回收后以该 mode 重开",
                        match existing {
                            crate::agent::browser::BrowserMode::Hidden => "hidden",
                            crate::agent::browser::BrowserMode::NewHeadless => "new_headless",
                            crate::agent::browser::BrowserMode::Headed => "headed",
                        },
                        match requested_mode {
                            crate::agent::browser::BrowserMode::Headed => "headed",
                            _ => "hidden",
                        },
                    ))
                } else {
                    None
                };
                (true, existing, hint)
            }
            None => (false, requested_mode, None),
        };
    let connect = str_arg(&args, "connect_url");
    let ua = str_arg(&args, "user_agent");
    // 窗口尺寸(第 100 轮):显式参数 clamp 到安全区间,缺省由驱动层决定
    // (headed=1920×1080 / hidden=1440×900)。
    let window_size = parse_window_size(&args);
    let highlight = args.get("highlight").and_then(Value::as_bool).unwrap_or(true);
    match BrowserManager::global()
        .new_page(url, effective_mode, connect, ua, window_size, highlight)
        .await
    {
        Ok((page_id, title, final_url)) => {
            if str_arg(&args, "wait_until") == Some("networkidle") {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
            let mut data = json!({
                "page_id": page_id,
                "title": title,
                "final_url": final_url,
                "reused": false,
                "mode": match effective_mode {
                    crate::agent::browser::BrowserMode::Hidden => "hidden",
                    crate::agent::browser::BrowserMode::NewHeadless => "new_headless",
                    crate::agent::browser::BrowserMode::Headed => "headed",
                },
                "browser_reused": browser_reused,
                "highlight": highlight && matches!(effective_mode, crate::agent::browser::BrowserMode::Headed),
                "window": {
                    "width": window_size.map(|(w, _)| w),
                    "height": window_size.map(|(_, h)| h),
                    "hint": "如需运行时调整:control(set_window);人工拖动窗口后:control(sync_viewport)",
                },
                // next_steps —— 分步引导,降低 LLM 编排成本(第 74 轮):
                // input_text → click → wait → elements 四步最常见动作链。
                "next_steps": open_next_steps(),
            });
            if let Some(arr) = args.get("block_resources").and_then(Value::as_array) {
                data["block_resources_hint"] = json!(arr);
            }
            if let Some(hint) = mode_hint {
                data["mode_hint"] = json!(hint);
            }
            envelope(0, "ok", data)
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains(NO_BROWSER_SENTINEL) {
                envelope(
                    3001,
                    "未检测到 Chrome/Edge/Chromium",
                    json!({"install":"请安装 Google Chrome 或 Microsoft Edge,或设置 LAEW_BROWSER_PATH"}),
                )
            } else {
                envelope(2001, &msg, json!({}))
            }
        }
    }
}

/// 列出当前存活页面(原 BrowserList);返回前自动清理失效 entry。
async fn run_list(_: Value) -> Result<String> {
    let pages: Vec<Value> = BrowserManager::global()
        .list_pages()
        .await
        .into_iter()
        .map(|(id, url, title, ts)| json!({"page_id":id,"url":url,"title":title,"created_at":ts}))
        .collect();
    envelope(0, "ok", json!({"pages": pages}))
}

/// 关闭指定 page_id(原 BrowserClose);最后一个页面关闭时回收内部浏览器进程
/// (launch 模式)。幂等。
/// 第 99 轮:`page_id="all"` 一键清场(关闭全部页面 + 回收浏览器,任务收尾用)。
async fn run_close(args: Value) -> Result<String> {
    let Some(id) = str_arg(&args, "page_id") else {
        return envelope(1001, "缺少 page_id", json!({}));
    };
    if id == "all" {
        let n = BrowserManager::global().list_pages().await.len();
        BrowserManager::global().shutdown().await;
        return envelope(0, "closed", json!({"page_id": "all", "closed": n}));
    }
    if BrowserManager::global().close_page(id).await {
        envelope(0, "closed", json!({"page_id":id}))
    } else {
        envelope(2000, "page_id 不存在", json!({"page_id":id}))
    }
}

// ===================== action=sequence(连续执行模式) =====================

/// 连续模式单批步骤上限:兼顾长任务编排与单次 tool_result 输出预算。
const MAX_SEQUENCE_STEPS: usize = 24;

/// 递归替换步骤里的 page_id 占位符。
fn resolve_page_placeholders(value: &mut Value, current: Option<&str>, spawned: Option<&str>) {
    match value {
        Value::String(s) => {
            if let Some(id) = current {
                *s = s.replace("${page_id}", id).replace("$page_id", id);
            }
            if let Some(id) = spawned {
                *s = s
                    .replace("${spawned_page_id}", id)
                    .replace("$spawned_page_id", id);
            }
        }
        Value::Array(items) => {
            for item in items {
                resolve_page_placeholders(item, current, spawned);
            }
        }
        Value::Object(map) => {
            for (_, item) in map.iter_mut() {
                resolve_page_placeholders(item, current, spawned);
            }
        }
        _ => {}
    }
}

/// 连续执行一组已明确的浏览器动作(连续执行模式)。
async fn run_sequence(args: Value) -> Result<String> {
    let Some(steps) = args.get("steps").and_then(Value::as_array) else {
        return envelope(1001, "缺少 steps 数组", json!({}));
    };
    if steps.is_empty() {
        return envelope(1001, "steps 不能为空", json!({}));
    }
    if steps.len() > MAX_SEQUENCE_STEPS {
        return envelope(
            1001,
            &format!("steps 数量超过上限({MAX_SEQUENCE_STEPS})"),
            json!({"count": steps.len(), "max": MAX_SEQUENCE_STEPS}),
        );
    }

    let stop_on_error = args
        .get("stop_on_error")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut current_page = str_arg(&args, "page_id").map(str::to_string);
    let mut last_spawned: Option<String> = None;
    let mut results: Vec<Value> = Vec::with_capacity(steps.len());
    let mut failed_at: Option<usize> = None;
    let mut first_error: Option<(i32, String)> = None;

    for (index, raw_step) in steps.iter().enumerate() {
        if !raw_step.is_object() {
            failed_at = Some(index);
            first_error = Some((1001, "step 必须是对象".into()));
            results.push(json!({
                "index": index + 1,
                "code": 1001,
                "message": "step 必须是对象",
                "data": {},
            }));
            break;
        }
        let mut step = raw_step.clone();
        let action = step
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if action == "sequence" {
            failed_at = Some(index);
            first_error = Some((1001, "sequence 步骤不允许嵌套 sequence".into()));
            results.push(json!({
                "index": index + 1,
                "action": action,
                "code": 1001,
                "message": "sequence 步骤不允许嵌套 sequence",
                "data": {},
            }));
            break;
        }

        let follow_spawned = step
            .get("follow_spawned")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if let Value::Object(ref mut map) = step {
            map.remove("follow_spawned");
        }
        resolve_page_placeholders(&mut step, current_page.as_deref(), last_spawned.as_deref());
        let control_action = step
            .get("control_action")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let step_page_id = str_arg(&step, "page_id").map(str::to_string);

        let output = McpWebUseTool.execute(step).await?;
        let parsed: Value = serde_json::from_str(&output).unwrap_or_else(
            |_| json!({"code": 2002, "message": "步骤返回非法 JSON", "data": {"raw": output}}),
        );
        let code = parsed.get("code").and_then(Value::as_i64).unwrap_or(2002) as i32;
        let message = parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let data = parsed.get("data").cloned().unwrap_or_else(|| json!({}));

        if code == 0 {
            if action == "open" {
                if let Some(id) = data.get("page_id").and_then(Value::as_str) {
                    current_page = Some(id.to_string());
                }
            } else if action == "control" {
                if control_action == "new_tab" {
                    if let Some(id) = data.get("spawned_page_id").and_then(Value::as_str) {
                        current_page = Some(id.to_string());
                    }
                }
            } else if action == "close" && current_page.as_deref() == step_page_id.as_deref() {
                current_page = None;
            }

            if let Some(spawned) = data.get("spawned_page_id").and_then(Value::as_str) {
                last_spawned = Some(spawned.to_string());
                if follow_spawned {
                    current_page = Some(spawned.to_string());
                }
            }
        }

        results.push(json!({
            "index": index + 1,
            "action": action,
            "code": code,
            "message": message,
            "data": data,
        }));
        if code != 0 {
            failed_at = Some(index);
            first_error = Some((code, message));
            if stop_on_error {
                break;
            }
        }
    }

    let completed = failed_at.is_none();
    let mut data = json!({
        "mode": "continuous",
        "steps": results,
        "step_count": steps.len(),
        "completed": completed,
        "stop_on_error": stop_on_error,
    });
    if let Some(index) = failed_at {
        data["failed_at"] = json!(index + 1);
    }
    if let Some(id) = &current_page {
        data["page_id"] = json!(id);
    }
    if let Some(id) = &last_spawned {
        data["spawned_page_id"] = json!(id);
    }

    match first_error {
        Some((code, message)) if !completed => {
            envelope(code, &format!("sequence 未完成:{message}"), data)
        }
        Some(_) | None => envelope(0, "sequence completed", data),
    }
}

// ===================== MCP_Web_Use 工具门面 =====================

/// 浏览器网页操控统一入口(MCP 风格单工具 + action 分发)。
pub struct McpWebUseTool;

/// 工具描述:同时承担「使用说明」职责(原 Chromium-WebUse Agent 系统提示词的
/// 作业规范部分精炼,全文见 SubAgent-Work 系统提示词的 MCP_Web_Use 段)。
const MCP_WEB_USE_DESCRIPTION: &str = r#"通过 CDP 驱动 Chromium 系浏览器操作网页(macOS / Windows / Linux,内存无头浏览器默认,也可接管已开浏览器;MCP 风格单工具多 action)。
用 action 参数选择操作:
- open(url*, mode?, reuse?, connect_url?, user_agent?, wait_until?, window_width?, window_height?, highlight?, timeout_ms?): 启动/接管 Chromium 并打开页面。默认纯 CDP 嵌入式无头浏览器(mode=hidden,无可见窗口,一次性临时 profile 不干扰日常浏览器);mode=headed 可见窗口(人工介入/可视化任务),默认 1920×1080(1080p),可用 window_width/window_height 自定义;highlight=true(默认)时 headed 窗口页面四周显示一圈蓝色选中边框+右上角「LAEW Agent 控制中」徽标,人工可一眼识别 Agent 控制的窗口;timeout_ms 控制页面加载超时(毫秒,默认 60000,内网慢速网站可加大)。connect_url 接管已用 --remote-debugging-port 启动的浏览器。同 URL 已有存活页面时默认复用(导航刷新,响应 reused:true 且 page_id 不变;reuse=false 强制新开);浏览器实例已存在时永远复用同一进程(响应 browser_reused:true + 真实 mode),不重复打开多个浏览器。返回 {page_id,title,final_url,reused,mode,browser_reused,window,next_steps}。未检测到浏览器返回 code=3001(确定性失败,如实告知用户安装引导,不要重试)。
- list(): 列出当前存活页面 [{page_id,url,title,created_at}];返回前自动清理失效 entry。冷启动后多轮任务优先用它同步页面索引。
- close(page_id*): 关闭指定页面;最后一个页面关闭时回收浏览器进程。page_id="all" 一键关闭全部页面并回收浏览器(任务收尾清场)。幂等;对话型页面(用户可能继续追问)可保留复用。
- control(page_id*, control_action*, params?): 全部写操作统一入口。control_action 枚举:click/human_click/right_click/double_click/hover/scroll/scroll_to/key_press/press_sequence/input_text/human_input/clear_input/upload_file/select_option/download/new_tab/close_tab/navigate/back/forward/reload/wait/eval_js/set_cookie/delete_cookie/set_storage/clear_storage/set_viewport/screenshot/heartbeat/drag/focus/blur/mouse_move/dispatch_event/set_window/sync_viewport/set_highlight/request_human。点击链接/new_tab 派生的新标签页经响应 spawned_page_id 回传,后续操作新页面必须用新 page_id。screenshot 一律落盘返回 save_path(看图片文字用 params.ocr=true,文本模型无法消费 base64);eval_js 直接写表达式,支持 return 与多语句(失败自动 IIFE 重试),超长返回值自动落盘并以 saved_to 引用;download 支持 http(s) url 或 selector、save_dir、filename、timeout_ms,data: URL 直接解码落盘,完成后返回绝对 save_path 与 byte_size。set_window 运行时调整真实浏览器窗口(width/height/left/top/window_state=maximized|fullscreen|minimized|normal,CDP setWindowBounds,调整后自动清除视口覆盖保证渲染自适应不缺区域);sync_viewport 在人工拖动窗口大小后调用,清除 device metrics 覆盖使视口=窗口内容区;set_highlight(enabled) 运行时开关蓝色选中边框;request_human(reason=captcha|sms|qr_login|login|manual_verify|custom, message?, options?, timeout_ms?=300000, bring_to_front?=true) 人工介入:滑块/短信验证码/扫码登录等无法自动跳过的流程,先请求人工在 TUI 选择/输入(code=0,human_response 为人工回答),人工取消返回 code=4002,超时或非交互式 TUI 模式返回 code=4001(如实告知用户改用交互模式重试,严禁伪造结果)。
- inspect(page_id*, info*, params?): 全部只读观察统一入口。info 枚举:console(控制台输出)/network(请求响应流)/elements(元素文本与矩形;params.selector 可选,缺失时默认返回 input/button/select/textarea/a/[role=button] 等全页交互元素)/dom(outerHTML 或节点树)/localstorage/sessionstorage/cookies/screenshot/page_meta/viewport/url/title/ping/image_urls/ocr(截图+OCR 识别图片文字,验证码/图表标签用;region 过滤词块)/blockers(启发式检测验证码/短信/扫码/登录墙等人工阻断,返回 blockers[]+suggested_action=request_human)。
- sequence(steps*, stop_on_error?): 连续执行模式。steps 最多 24 个,每项结构与单步调用相同(open/list/close/control/inspect),禁止嵌套 sequence;批内 page_id 用 "$page_id"/"${page_id}" 占位,点击派生新页可用 "$spawned_page_id"/"${spawned_page_id}",默认自动跟随 spawned_page_id,单步可 follow_spawned=false 保持原页。响应逐步返回 code/message/data,并给出最终 page_id。

【两种工作模式】1) 单步执行模式:直接调用 open/control/inspect/list/close,一次一个动作,适合探索、调试和高风险操作;2) 连续执行模式:先用单步 inspect(elements/dom/console/network)探索结构,再 action=sequence 一次执行已明确动作链,适合流程稳定任务(登录/表单类:inspect(form) → ocr 验证码 → sequence(input×N + click + wait + verify) 一次打包)。两种模式可混合、可多次调用。
【标准作业顺序】open 拿 page_id → inspect 探索真实 DOM → control 执行动作 → inspect 验证结果 → 任务完成后 close 释放(确定不再需要的页面;全部结束用 page_id="all" 清场)。
【错误码对策】1001 修正参数;2000 page_id 失效→action=list 重新同步;2001 断连→重新 open;2002 换 selector 或 input_text 的 use_js 路径重试;3001 未安装浏览器→如实告知用户,不要编造结果。
【人工介入(HITL)】遇到滑块/图形验证码(OCR 不可读)/短信验证码/扫码登录/人脸核身等无法自动完成的流程:可视化场景先 open(mode=headed) 让人工看到窗口(蓝色边框标识),再 control(request_human, reason=..., message=说明要人工做什么, options=[...]) 在 TUI 发起人工选择;code=0 用 data.human_response 继续(短信验证码数字人工直接在 TUI 输入);4001=超时/非交互模式,如实报告;4002=人工取消,终止该路径。切 hidden→headed 需 close("all") 回收后重开。
【作业要点】中文输入优先 params.use_js=true(React/Vue 受控组件兼容);复杂页面先 inspect(info=elements) 探测真实 DOM 再操作,不要硬猜 selector;AI 对话类网站回复等待用 control(wait, selector=[class*=response]..., timeout_ms=60000);看图片里的文字(验证码/图表标签)一律 screenshot(params.ocr=true) 或 inspect(info=ocr),禁止 Read 图片文件、禁止用 Bash/python/tesseract 解码图片(文本模型无视觉,纯浪费迭代);验证码读码后不要刷新页面或点击验证码图(刷新即换码),提交报验证码错误才点图刷新重读;DOM 提取注意 truncated 标记分段。
【安全红线】支付/删除/确认提交/登出等不可逆或高风险动作禁止放进 sequence,必须单步执行并检查页面状态;登录凭证只填用户明确提供的账号密码,不要编造;OCR 不可用的平台上验证码类任务如实报告等待人工,禁止猜测验证码;只读优先——能 inspect 回答的问题不做任何写操作。"#;

#[async_trait]
impl Tool for McpWebUseTool {
    fn name(&self) -> &str {
        MCP_WEB_USE_TOOL_NAME
    }

    fn description(&self) -> &str {
        MCP_WEB_USE_DESCRIPTION
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "list", "close", "control", "inspect", "sequence"],
                    "description": "要执行的浏览器操作:open(启动/接管浏览器并打开页面) / list(列出存活页面) / close(关闭页面) / control(写操作统一入口) / inspect(只读观察统一入口) / sequence(连续执行一批操作)"
                },
                "url": { "type": "string", "description": "open 必填:目标网址" },
                "reuse": { "type": "boolean", "default": true, "description": "open 可选:同 URL 已有存活页面时复用(导航刷新,page_id 不变,响应 reused:true);false 强制新开页面" },
                "mode": { "type": "string", "enum": ["hidden", "new_headless", "headed"], "default": "hidden", "description": "open 可选:浏览器模式;hidden=纯 CDP 无窗口(默认,推荐),new_headless=旧 headless=true,headed=可见窗口(调试截图)" },
                "window_width": { "type": "integer", "minimum": 320, "maximum": 7680, "description": "open 可选:浏览器窗口宽(px);缺省 headed=1920(1080p)/hidden=1440;与 window_height 成对使用;浏览器已存在时仅记录请求值(单实例复用)" },
                "window_height": { "type": "integer", "minimum": 240, "maximum": 4320, "description": "open 可选:浏览器窗口高(px);缺省 headed=1080(1080p)/hidden=900;运行时调整用 control(set_window),人工拖动后用 control(sync_viewport) 自适应" },
                "highlight": { "type": "boolean", "default": true, "description": "open 可选:headed 模式在页面四周注入一圈蓝色选中边框+右上角「LAEW Agent 控制中」徽标(标识 Agent 控制的窗口,人工介入用);pointer-events:none 不影响页面交互;可用 control(set_highlight, enabled=false) 运行时关闭" },
                "timeout_ms": { "type": "integer", "description": "open 可选:页面加载超时(毫秒),默认 60000" },
                "connect_url": { "type": "string", "description": "open 可选:接管已开浏览器,如 http://127.0.0.1:9222(需 --remote-debugging-port 启动)" },
                "user_agent": { "type": "string", "description": "open 可选:覆盖 User-Agent" },
                "wait_until": { "type": "string", "enum": ["load", "domcontentloaded", "networkidle"], "description": "open 可选:打开后额外等待(networkidle 额外等 1.5s)" },
                "block_resources": { "type": "array", "items": { "type": "string" }, "description": "open 可选:拦截资源类型(image/stylesheet/font/media/script),当前仅记录提示" },
                "page_id": { "type": "string", "description": "close/control/inspect 必填:open/list 返回或 spawned_page_id 回传的页面句柄;close 时可为 \"all\" 一键关闭全部页面并回收浏览器" },
                "control_action": {
                    "type": "string",
                    "enum": [
                        "click", "human_click", "right_click", "double_click", "hover",
                        "scroll", "scroll_to", "key_press", "press_sequence",
                        "input_text", "human_input", "clear_input",
                        "upload_file", "select_option", "download",
                        "new_tab", "close_tab",
                        "navigate", "back", "forward", "reload",
                        "wait", "eval_js",
                        "set_cookie", "delete_cookie",
                        "set_storage", "clear_storage", "set_viewport",
                        "screenshot", "heartbeat",
                        "drag", "focus", "blur", "mouse_move", "dispatch_event",
                        "set_window", "sync_viewport", "set_highlight", "request_human"
                    ],
                    "description": "control 必填:具体写操作(鼠标/键盘/输入/上传/下载/标签页/导航/等待/JS/Cookie/Storage/视口/截图等 39 个;set_window/sync_viewport=窗口自适应,request_human=人工介入)"
                },
                "info": {
                    "type": "string",
                    "enum": [
                        "console", "network", "elements", "dom",
                        "localstorage", "sessionstorage", "cookies",
                        "screenshot", "page_meta", "viewport",
                        "url", "title", "ping", "image_urls", "ocr", "blockers"
                    ],
                    "description": "inspect 必填:观察维度(Console 输出 / Network 流 / Elements 元素 / DOM / localStorage 等 16 个);ocr=截图+OCR 识别图片文字(验证码/图表标签,region 过滤词块);blockers=检测验证码/短信/扫码/登录墙等人工阻断"
                },
                "params": { "type": "object", "description": "control/inspect 可选:动作参数对象(selector/text/key/keys/timeout_ms/url/x/y/file_paths/save_dir/filename 等按 control_action/info 各异)。screenshot/ocr 支持 save_path(落盘路径)、ocr=true(返回 OCR 文字)、region{x,y,width,height}(OCR 词块过滤)、return_base64=true(显式内联 base64,默认不返回)、full_page/format/quality" },
                "execution_mode": {
                    "type": "string",
                    "enum": ["single_step", "continuous"],
                    "default": "single_step",
                    "description": "使用模式说明。single_step=默认,一次调用一个 action,适合探索/调试/高风险操作;continuous=action=sequence,先探索后在 steps 中一次执行动作链,适合稳定流程。两种模式可混合多次调用"
                },
                "steps": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 24,
                    "description": "sequence 必填:连续执行步骤。每项结构与单步入参相同(action/page_id/control_action/info/params),可用 $page_id、$spawned_page_id 占位符;单步级 follow_spawned=false 可禁止自动跟随新标签页",
                    "items": { "type": "object", "additionalProperties": true }
                },
                "stop_on_error": { "type": "boolean", "default": true, "description": "sequence 可选:默认 true,任一步失败立即停止;false 用于采集全量执行报告" }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let Some(action) = str_arg(&args, "action") else {
            return envelope(1001, "缺少 action", json!({}));
        };
        match action {
            "open" => run_open(args).await,
            "list" => run_list(args).await,
            "close" => run_close(args).await,
            "control" => control::run(args).await,
            "inspect" => inspect::run(args).await,
            "sequence" => run_sequence(args).await,
            other => envelope(1001, "未知 action", json!({"action": other})),
        }
    }
}
