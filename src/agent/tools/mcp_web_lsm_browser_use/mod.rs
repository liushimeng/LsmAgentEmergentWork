//! MCP_Web_LsmAgentBrowser_Use:浏览器网页操控的统一 MCP 风格入口(LsmAgentBrowser 兼容层)。
//!
//! 对外契约(action / control_action / info / 参数 / 信封 / 错误码)保持不变;
//! 底层由进程内自研内核 `agent::browser` 提供,无 Chrome、无 CDP、无外部浏览器进程。
//!
//! - 简单档:open → control/inspect 多轮 → close;
//! - 连续档:sequence / batch 批量步骤;explore 批量观测;
//! - `page_id` 全生命周期:open 获取,control/inspect 复用,派生页经 `spawned_page_id` 回传;
//! - 所有动作返回 `{code,message,data}`;错误码供 Agent 机械决策。
//!
//! 设计见 `docs/自定义浏览器和V8引擎/03-MCP_Web_LsmAgentBrowser_Use兼容层设计.md`。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Tool;
use crate::agent::browser::{BrowserMode, EngineError, LsmBrowserManager};
use crate::error::Result;

mod control;
mod inspect;
pub(crate) mod visit_ledger;
#[cfg(test)]
mod tests;

/// 工具名(LLM 可见的唯一浏览器操控入口)。
pub const MCP_WEB_LSM_BROWSER_USE_TOOL_NAME: &str = "MCP_Web_LsmAgentBrowser_Use";

pub(super) const CODE_TARGET_ANCHOR_VIOLATION: i32 = 6001;

/// 统一 JSON 信封。
pub(super) fn envelope(code: i32, message: &str, data: Value) -> Result<String> {
    Ok(json!({"code": code, "message": message, "data": data}).to_string())
}

pub(super) fn engine_error(err: EngineError) -> String {
    json!({"code": err.code, "message": err.message, "data": err.data}).to_string()
}

pub(super) fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

pub(super) fn bool_arg(args: &Value, key: &str, default: bool) -> bool {
    match args.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes" | "on"
        ),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(default),
        _ => default,
    }
}

pub(super) fn u64_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

/// 页面句柄解析:顶层 / `params` 双位置取值;缺失时按「唯一存活页」自动选中。
///
/// 与 Chrome 侧 `mcp_chrome_use::resolve_page_id` 同语义(2026-10-26 实测补齐):
/// 单页任务里 page_id 是纯冗余参数,强制必填只制造失败面(实测漏传一次白费
/// 一整轮 LLM 往返)。多页时返回错误并附存活页清单,避免模型继续盲猜。
pub(super) async fn resolve_page_id(
    args: &Value,
) -> std::result::Result<(String, bool), String> {
    let p = args.get("params").filter(|v| v.is_object()).unwrap_or(args);
    if let Some(id) = str_arg(args, "page_id").or_else(|| str_arg(p, "page_id")) {
        return Ok((id.to_string(), false));
    }
    let live = McpWebLsmBrowserUseTool::mgr().list_pages().await;
    match live.len() {
        1 => Ok((live[0].page_id.clone(), true)),
        0 => Err("缺少 page_id,且当前没有存活页面;请先 action=open 打开页面".to_string()),
        _ => {
            let desc: Vec<Value> = live
                .iter()
                .take(8)
                .map(|p| {
                    json!({
                        "page_id": p.page_id,
                        "url": crate::agent::web_activity::clip_chars(&p.url, 80),
                    })
                })
                .collect();
            Err(format!(
                "缺少 page_id(当前有 {} 个存活页面,请指定其一);存活页:{}",
                live.len(),
                serde_json::to_string(&desc).unwrap_or_default()
            ))
        }
    }
}

/// 任务锚点:目标站外导航阻断(兼容旧 6001 契约)。
pub(super) fn target_anchor_guard(url: &str) -> Option<Result<String>> {
    let v = crate::agent::safety::check_open_against_target_anchor(url)?;
    tracing::warn!(
        requested_host = %v.host,
        allowed_hosts = ?v.allowed_hosts,
        "MCP_Web_LsmAgentBrowser_Use 目标站点越界已阻断(任务锚点,code=6001)"
    );
    Some(envelope(
        CODE_TARGET_ANCHOR_VIOLATION,
        "目标站点越界(任务锚点约束)",
        json!({
            "requested_url": url,
            "requested_host": v.host,
            "allowed_hosts": v.allowed_hosts,
            "anchor_evidence": v.evidence,
            "required_action": "如实报告「目标站点不可达或未指定」并结束本单元;不得改用其它站点替代。",
        }),
    ))
}

/// 从工具输出文本里提取 `page_id`(兼容旧 Agent 循环的 open 后续引导)。
pub fn extract_page_id_from_text(text: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if let Some(pid) = v
            .pointer("/data/page_id")
            .and_then(Value::as_str)
            .or_else(|| v.get("page_id").and_then(Value::as_str))
        {
            if !pid.is_empty() {
                return Some(pid.to_string());
            }
        }
    }
    // 兜底:`p_` + 8 位十六进制。
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 10 <= bytes.len() {
        if bytes[i] == b'p'
            && bytes[i + 1] == b'_'
            && bytes[i + 2..i + 10].iter().all(u8::is_ascii_hexdigit)
        {
            return Some(text[i..i + 10].to_string());
        }
        i += 1;
    }
    None
}

fn open_next_steps() -> Value {
    json!([
        {"step": 1, "action": "control", "control_action": "input_text", "params": {"selector": "输入框选择器", "text": "关键词"}},
        {"step": 2, "action": "control", "control_action": "click", "params": {"selector": "搜索/提交按钮选择器"}},
        {"step": 3, "action": "control", "control_action": "wait", "params": {"selector": "结果条目选择器", "state": "visible", "network_idle": 800}},
        {"step": 4, "action": "inspect", "info": "elements", "params": {"selector": "结果条目选择器"}},
    ])
}

/// MCP_Web_LsmAgentBrowser_Use 工具。
pub struct McpWebLsmBrowserUseTool;

impl McpWebLsmBrowserUseTool {
    fn mgr() -> std::sync::Arc<LsmBrowserManager> {
        LsmBrowserManager::global()
    }
}

#[async_trait]
impl Tool for McpWebLsmBrowserUseTool {
    fn name(&self) -> &str {
        MCP_WEB_LSM_BROWSER_USE_TOOL_NAME
    }

    fn description(&self) -> &str {
        "浏览器网页操控统一入口(自研内核 LsmAgentBrowser,进程内运行,无外部浏览器/CDP)。\
action=open 打开页面拿 page_id;list 列出页面;control 执行写操作(click/input_text/key_press/\
scroll/navigate/back/forward/reload/wait/eval_js/screenshot/download/set_cookie/set_storage/\
new_tab/new_window/close_tab/bring_to_front/request_human 等);inspect 观测页面(title/url/text/\
html/elements/links/forms/cookies/localstorage/network/console/viewport/extract/extract_links/\
page_state/blockers/screenshot/ocr 等);close 释放页面;sequence/batch 连续执行 ≤24 步;\
explore 一次做 ≤8 个观测。\
wait 是就绪判定原语:可给 selector+state(visible/hidden/attached/detached)、text(元素文本包含)、\
js(页面表达式为真)、network_idle(网络静默毫秒),多条件取「与」,超时返回 2002。\
★ 搜地址/填地点时,input_text 建议带 params.submit=true:输入后自动回车,\
把「联想下拉里的规范 POI」提交出去;只输入不回车,站点会用原始文本做地理编码,\
结果页直接报「未找到相关地点」(实测百度地图路线规划)。\
返回统一信封 {code,message,data}。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "list", "close", "control", "inspect", "sequence", "explore", "batch"]
                },
                "url": { "type": "string", "description": "open/explore 自动打开的目标网址" },
                "force": { "type": "boolean", "default": false, "description": "兼容参数;新内核对导航不做节奏闸" },
                "new_window": { "type": "boolean", "default": false, "description": "open 时在独立逻辑窗口打开" },
                "reuse": { "type": "boolean", "default": true, "description": "open 时同 URL/同站已有页面则复用" },
                "mode": { "type": "string", "enum": ["headed", "hidden", "new_headless"], "default": "headed",
                          "description": "浏览器模式。headed = 进程内内核 + 独立原生窗口查看器(用户可直接看到浏览器窗口,含后退/前进/刷新/地址栏);hidden = 纯无头无窗口。需要让用户看到浏览器运行过程时用 headed(默认)" },
                "window_width": { "type": "integer", "minimum": 320, "maximum": 7680 },
                "window_height": { "type": "integer", "minimum": 240, "maximum": 4320 },
                "highlight": { "type": "boolean", "default": true },
                "guard": { "type": "string", "enum": ["locked", "open", "partial"], "default": "locked" },
                "stealth": { "type": "string", "enum": ["fidelity", "hardened", "off"], "default": "hardened" },
                "allow_selectors": { "type": "array", "items": { "type": "string" }, "maxItems": 8 },
                "block_selectors": { "type": "array", "items": { "type": "string" }, "maxItems": 8 },
                "guard_note": { "type": "string", "maxLength": 60 },
                "overlay": { "type": "boolean", "default": true },
                "timeout_ms": { "type": "integer", "description": "open 加载超时,默认 60000" },
                "connect_url": { "type": "string", "description": "兼容参数;新内核无外部浏览器可接管,返回 3002" },
                "reuse_existing": { "type": "boolean", "default": false, "description": "兼容参数;新内核返回 3002" },
                "auto_relaunch": { "type": "boolean", "default": false },
                "user_agent": { "type": "string" },
                "wait_until": { "type": "string", "enum": ["load", "domcontentloaded", "networkidle"] },
                "block_resources": { "type": "array", "items": { "type": "string" } },
                "page_id": { "type": "string", "description": "close/control/inspect 必填;close 可为 all/background/others" },
                "keep": { "type": "string", "description": "close(page_id=others) 时保留的 page_id" },
                "control_action": {
                    "type": "string",
                    "enum": [
                        "click", "human_click", "right_click", "double_click", "hover",
                        "scroll", "scroll_to", "scroll_to_text", "key_press", "press_sequence",
                        "input_text", "human_input", "clear_input", "select_option", "select_dropdown",
                        "upload_file", "download", "new_tab", "new_window", "close_tab", "bring_to_front",
                        "navigate", "back", "forward", "reload", "wait", "eval_js",
                        "set_cookie", "delete_cookie", "set_storage", "clear_storage",
                        "set_viewport", "screenshot", "heartbeat", "drag", "focus", "blur",
                        "mouse_move", "dispatch_event", "set_window", "sync_viewport",
                        "set_highlight", "set_overlay", "set_guard", "auto_follow",
                        "set_stealth", "request_human"
                    ]
                },
                "info": {
                    "type": "string",
                    "enum": [
                        "console", "network", "elements", "dom", "localstorage", "sessionstorage",
                        "cookies", "screenshot", "page_meta", "viewport", "url", "title", "ping",
                        "image_urls", "ocr", "blockers", "visibility", "extract_links", "extract",
                        "page_state", "stylesheets", "coverage", "text", "page_health"
                    ]
                },
                "params": { "type": "object", "description": "control/inspect 的动作参数" },
                "execution_mode": { "type": "string", "enum": ["single_step", "continuous"], "default": "single_step" },
                "steps": { "type": "array", "minItems": 1, "maxItems": 24, "items": { "type": "object" } },
                "stop_on_error": { "type": "boolean", "default": true },
                "queries": { "type": "array", "minItems": 1, "maxItems": 8, "items": { "type": "object" } },
                "summary_hint": { "type": "string" },
                "summary": { "type": "string" },
                "message": { "type": "string", "maxLength": 600 },
                "reason": { "type": "string" },
                "options": { "type": "array", "items": { "type": "string" }, "maxItems": 8 }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let Some(action) = str_arg(&args, "action").map(str::to_string) else {
            return envelope(1001, "缺少 action", json!({}));
        };
        // 屏蔽页面状态栏/进度栏:动作前后落账(docs/双浏览器工具架构/02)。
        // begin 用入参 page_id 精准落账(缺省广播),end 优先取输出里的新 page_id(open)。
        let label = describe_lsm_action(&args);
        let page_hint = str_arg(&args, "page_id").map(str::to_string);
        let mgr = Self::mgr();
        mgr.note_action_begin(page_hint.as_deref(), &label).await;
        let started = std::time::Instant::now();
        let out = match action.as_str() {
            "open" => run_open(args).await,
            "list" => run_list().await,
            "close" => run_close(args).await,
            "control" => control::run(args).await,
            "inspect" => inspect::run(args).await,
            "sequence" | "batch" => run_sequence(args).await,
            "explore" => run_explore(args).await,
            other => envelope(1001, "未知 action", json!({"action": other})),
        };
        let ok = out.as_deref().ok().and_then(envelope_code).map(|c| c == 0).unwrap_or(false);
        let effective = out
            .as_deref()
            .ok()
            .and_then(envelope_page_id)
            .or(page_hint);
        mgr.note_action_end(effective.as_deref(), &label, ok, started.elapsed().as_millis() as u64)
            .await;
        out
    }
}

/// 把 Lsm 工具动作参数渲染成人读标签(进度栏第一行;与 Chrome 侧
/// `chrome_browser::status::describe_web_action` 同款约定)。
fn describe_lsm_action(args: &Value) -> String {
    let action = str_arg(args, "action").unwrap_or("?");
    let mut out = action.to_string();
    match action {
        "open" => {
            if let Some(u) = str_arg(args, "url") {
                out.push(' ');
                out.push_str(u);
            }
        }
        "control" => {
            if let Some(ca) = str_arg(args, "control_action") {
                out.push(' ');
                out.push_str(ca);
            }
            if let Some(p) = args.get("params") {
                for key in ["selector", "url", "text", "script", "key"] {
                    if let Some(v) = p.get(key).and_then(Value::as_str) {
                        out.push(' ');
                        out.push_str(key);
                        out.push('=');
                        out.push_str(&crate::agent::web_activity::clip_chars(v, 40));
                        break;
                    }
                }
            }
        }
        "inspect" => {
            if let Some(i) = str_arg(args, "info") {
                out.push(' ');
                out.push_str(i);
            }
        }
        "sequence" | "batch" => {
            let n = args.get("steps").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
            out.push_str(&format!(" {n} 步"));
        }
        "explore" => {
            let n = args.get("queries").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
            out.push_str(&format!(" {n} 个维度"));
        }
        "close" => {
            if let Some(p) = str_arg(args, "page_id") {
                out.push(' ');
                out.push_str(p);
            }
        }
        _ => {}
    }
    crate::agent::web_activity::clip_chars(&out, 90)
}

/// 从统一信封文本里取 `code`(解析失败按 None,不影响主流程)。
fn envelope_code(out: &str) -> Option<i64> {
    serde_json::from_str::<Value>(out).ok()?.get("code")?.as_i64()
}

/// 从统一信封文本里取 `data.page_id`(open 的产物落账用)。
fn envelope_page_id(out: &str) -> Option<String> {
    let pid = serde_json::from_str::<Value>(out)
        .ok()?
        .get("data")?
        .get("page_id")?
        .as_str()?
        .to_string();
    Some(pid)
}

// ===================== open / list / close =====================

fn merge_open_params(args: &mut Value) {
    if str_arg(args, "url").is_some() {
        return;
    }
    let Some(Value::Object(params)) = args.get("params").cloned() else {
        return;
    };
    if let Value::Object(top) = args {
        for (k, v) in params {
            top.entry(k).or_insert(v);
        }
    }
}

pub(super) async fn run_open(mut args: Value) -> Result<String> {
    merge_open_params(&mut args);
    let Some(url) = str_arg(&args, "url").map(str::to_string) else {
        return envelope(1001, "缺少 url(顶层或 params.url)", json!({}));
    };
    if let Some(blocked) = target_anchor_guard(&url) {
        return blocked;
    }
    let reuse_existing = bool_arg(&args, "reuse_existing", false)
        || str_arg(&args, "connect_url").is_some();
    if reuse_existing {
        return envelope(
            3002,
            "新内核不支持接管外部浏览器",
            json!({
                "connect_url": str_arg(&args, "connect_url"),
                "reuse_existing": true,
                "hint": "LsmAgentBrowser 在进程内运行,没有 Chrome/CDP 可供接管。请直接 open(url);\
                         登录态可用 control(request_human) 请人工完成,或 control(set_cookie/set_storage) 注入。",
            }),
        );
    }
    let new_window = bool_arg(&args, "new_window", false);
    let mode = BrowserMode::from_env_or_arg(str_arg(&args, "mode"));
    let width = u64_arg(&args, "window_width").map(|v| v as u32);
    let height = u64_arg(&args, "window_height").map(|v| v as u32);
    let reuse = bool_arg(&args, "reuse", true);
    let timeout = u64_arg(&args, "timeout_ms").unwrap_or(60_000).clamp(1_000, 600_000);
    let ua = str_arg(&args, "user_agent");
    let mgr = McpWebLsmBrowserUseTool::mgr();
    let had_pages = mgr.has_browser().await;
    let out = match mgr
        .new_page(&url, mode, ua, width, height, new_window, reuse, timeout)
        .await
    {
        Ok(v) => v,
        Err(e) => return Ok(engine_error(e)),
    };
    visit_ledger::record_visit(
        &out.page_id,
        &out.final_url,
        &out.title,
        visit_ledger::VisitSource::Open,
    );
    let guard_mode = str_arg(&args, "guard").unwrap_or("locked");
    let mut data = json!({
        "page_id": out.page_id,
        "title": out.title,
        "final_url": out.final_url,
        "request_url": out.request_url,
        "reused": out.reused,
        "mode": out.mode,
        "browser_reused": had_pages,
        "connect_mode": false,
        "window_id": out.window_id,
        "status": out.status,
        "content_type": out.content_type,
        "byte_size": out.source_bytes,
        "redirect_chain": out.redirect_chain,
        "highlight": bool_arg(&args, "highlight", true),
        "guard": {
            "mode": guard_mode,
            "allow_selectors": args.get("allow_selectors").cloned().unwrap_or(json!([])),
            "block_selectors": args.get("block_selectors").cloned().unwrap_or(json!([])),
            "note": str_arg(&args, "guard_note"),
            "supported": false,
            "hint": "阶段一无原生窗口,guard 仅记录状态",
        },
        "overlay": {"enabled": false, "input_locked": false, "visual_mask": false,
                     "hint": "进程内无头内核无可视化蒙层"},
        "window": {
            "requested": {"width": width, "height": height},
            "effective": {"width": out.viewport_width, "height": out.viewport_height},
        },
        "next_steps": open_next_steps(),
    });
    if let Some(hint) = out.mode_hint {
        data["mode_hint"] = json!(hint);
    }
    if let Some(note) = visit_ledger::visit_note(&out.final_url) {
        data["visit_note"] = note;
    }
    if out.final_url.trim().eq_ignore_ascii_case("about:blank") {
        data["page_health"] = json!({"blank": true, "hint": "落地为空白页;可检查 URL 或重新 open"});
    }
    // 能力缺口如实告知:站点申请过 WebGL 上下文(百度地图等三维/地图站点),
    // 而本内核无 GPU 后端 → 地图底图画不出来。不说清楚,Agent 会对同一片
    // 空白反复截图/重试,白烧好几轮。
    if out.webgl_asked > 0 {
        data["capability_gaps"] = json!([{
            "kind": "webgl",
            "requested": out.webgl_asked,
            "detail": "站点申请过 WebGL 上下文(常见于地图/三维场景);LsmAgentBrowser 内核无 WebGL,                       该区域不会绘制出像素(底图留空/仅剩 DOM 控件)。",
            "hint": "需要像素级地图底图时改用 MCP_Web_Chrome_Use(真 Chromium 渲染);                     本工具仍可用于读取页面 DOM 文本(如路线方案列表)。",
        }]);
    }
    envelope(0, "ok", data)
}

async fn run_list() -> Result<String> {
    let mgr = McpWebLsmBrowserUseTool::mgr();
    let pages = mgr.list_pages().await;
    let windows: std::collections::HashSet<String> =
        pages.iter().map(|p| p.window_id.clone()).collect();
    let items: Vec<Value> = pages
        .iter()
        .map(|p| {
            json!({
                "page_id": p.page_id,
                "url": p.url,
                "title": p.title,
                "created_at": p.created_at,
                "foreground": p.foreground,
                "window_id": p.window_id,
            })
        })
        .collect();
    let mut data = json!({
        "pages": items,
        "count": pages.len(),
        "windows": windows.len(),
    });
    if pages.len() >= 6 {
        data["tabs_hint"] = json!("页面较多,完成读取后及时 close 不需要的 page_id");
    }
    envelope(0, "ok", data)
}

async fn run_close(args: Value) -> Result<String> {
    let Some(page_id) = str_arg(&args, "page_id").map(str::to_string) else {
        return envelope(1001, "close 需要 page_id(all/background/others/具体 id)", json!({}));
    };
    let mgr = McpWebLsmBrowserUseTool::mgr();
    if page_id == "others" {
        let Some(keep) = str_arg(&args, "keep") else {
            return envelope(1001, "close(page_id=others) 需要 keep", json!({}));
        };
        if !mgr.page_exists(keep).await {
            return envelope(2000, "keep 页面不存在", json!({"page_id": keep}));
        }
        let out = mgr.close_others(keep).await;
        return envelope(
            0,
            "closed",
            json!({"page_id": "others", "keep": keep, "closed": out.closed, "count": out.closed.len()}),
        );
    }
    let out = mgr.close_page(&page_id).await;
    match page_id.as_str() {
        "all" => envelope(
            0,
            "closed",
            json!({"page_id": "all", "closed": out.closed.len()}),
        ),
        "background" => envelope(
            0,
            "closed",
            json!({"page_id": "background", "closed": out.closed, "count": out.closed.len()}),
        ),
        _ => {
            if out.closed.is_empty() {
                envelope(2000, "page_id 不存在或已关闭", json!({"page_id": page_id}))
            } else {
                envelope(0, "closed", json!({"page_id": page_id}))
            }
        }
    }
}

// ===================== sequence / batch / explore =====================

fn substitute(value: &Value, current: &str, spawned: &str) -> Value {
    match value {
        Value::String(s) => {
            let s = s
                .replace("${page_id}", current)
                .replace("$page_id", current)
                .replace("${spawned_page_id}", spawned)
                .replace("$spawned_page_id", spawned);
            Value::String(s)
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| substitute(v, current, spawned)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), substitute(v, current, spawned)))
                .collect(),
        ),
        other => other.clone(),
    }
}

async fn dispatch_action(action: &str, args: Value) -> Result<String> {
    // sequence/batch 步内动作同样落账(进度栏逐步滚动显示,docs/双浏览器工具架构/02)。
    let label = describe_lsm_action(&args);
    let page_hint = str_arg(&args, "page_id").map(str::to_string);
    let mgr = McpWebLsmBrowserUseTool::mgr();
    mgr.note_action_begin(page_hint.as_deref(), &label).await;
    let started = std::time::Instant::now();
    let out = match action {
        "open" => run_open(args).await,
        "list" => run_list().await,
        "close" => run_close(args).await,
        "control" => control::run(args).await,
        "inspect" => inspect::run(args).await,
        other => envelope(1001, "sequence 步内 action 非法", json!({"action": other})),
    };
    let ok = out.as_deref().ok().and_then(envelope_code).map(|c| c == 0).unwrap_or(false);
    let effective = out
        .as_deref()
        .ok()
        .and_then(envelope_page_id)
        .or(page_hint);
    mgr.note_action_end(effective.as_deref(), &label, ok, started.elapsed().as_millis() as u64)
        .await;
    out
}

async fn run_sequence(args: Value) -> Result<String> {
    let Some(steps) = args.get("steps").and_then(Value::as_array).cloned() else {
        return envelope(1001, "sequence 需要 steps", json!({}));
    };
    if steps.is_empty() || steps.len() > 24 {
        return envelope(
            1001,
            "steps 数量必须在 1..=24",
            json!({"count": steps.len(), "max": 24}),
        );
    }
    let stop_on_error = bool_arg(&args, "stop_on_error", true);
    let top_page = str_arg(&args, "page_id").unwrap_or("").to_string();
    let mut current = top_page.clone();
    let mut spawned = String::new();
    let mut results = Vec::new();
    let mut failed_at: Option<usize> = None;
    let mut first_error: Option<i32> = None;

    for (idx, raw_step) in steps.iter().enumerate() {
        let step = substitute(raw_step, &current, &spawned);
        let Some(obj) = step.as_object() else {
            results.push(json!({"index": idx + 1, "code": 1001, "message": "step 必须是对象", "data": {}}));
            failed_at = Some(idx + 1);
            first_error = Some(1001);
            break;
        };
        let mut step_args = Value::Object(obj.clone());
        if step_args.get("action").and_then(Value::as_str).is_none() {
            let inferred = if step_args.get("control_action").is_some() {
                Some("control")
            } else if step_args.get("info").is_some() {
                Some("inspect")
            } else {
                None
            };
            match inferred {
                Some(a) => {
                    step_args["action"] = json!(a);
                }
                None => {
                    results.push(json!({"index": idx + 1, "code": 1001, "message": "step 缺少 action(且无法由 control_action/info 推断)", "data": {}}));
                    failed_at = Some(idx + 1);
                    first_error = Some(1001);
                    break;
                }
            }
        }
        let action = step_args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if action == "sequence" || action == "batch" {
            results.push(json!({"index": idx + 1, "code": 1001, "message": "禁止嵌套 sequence/batch", "data": {}}));
            failed_at = Some(idx + 1);
            first_error = Some(1001);
            break;
        }
        // page_id 继承:step > top-level > current(派生页)
        let step_pid = step_args
            .get("page_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && !s.starts_with('$'))
            .map(str::to_string);
        if step_pid.is_none() && !current.is_empty() && action != "open" && action != "list" {
            step_args["page_id"] = json!(current);
        }
        let follow_spawned = step_args
            .get("follow_spawned")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if let Value::Object(o) = &mut step_args {
            o.remove("follow_spawned");
        }

        let is_human = action == "control"
            && step_args.get("control_action").and_then(Value::as_str) == Some("request_human");
        let start = std::time::Instant::now();
        let out = if is_human {
            dispatch_action(&action, step_args).await
        } else {
            match tokio::time::timeout(
                std::time::Duration::from_secs(150),
                dispatch_action(&action, step_args),
            )
            .await
            {
                Ok(v) => v,
                Err(_) => Ok(json!({
                    "code": 2002,
                    "message": "sequence 单步超过 150s 已中止",
                    "data": {"required_action": "拆分步骤或改用 control(wait) 分次等待"},
                })
                .to_string()),
            }
        };
        let text = out.unwrap_or_else(|e| {
            json!({"code": 2001, "message": e.to_string(), "data": {}}).to_string()
        });
        let parsed: Value = serde_json::from_str(&text).unwrap_or_else(|_| {
            json!({"code": 2002, "message": "步骤返回非 JSON", "data": {}})
        });
        let code = parsed.get("code").and_then(Value::as_i64).unwrap_or(2002) as i32;
        let msg = parsed.get("message").and_then(Value::as_str).unwrap_or("");
        let data = parsed.get("data").cloned().unwrap_or(json!({}));
        if let Some(p) = data.get("page_id").and_then(Value::as_str) {
            if !p.is_empty() {
                current = p.to_string();
            }
        }
        if let Some(p) = data.get("spawned_page_id").and_then(Value::as_str) {
            if !p.is_empty() {
                spawned = p.to_string();
                if follow_spawned {
                    current = spawned.clone();
                }
            }
        }
        let mut step_result = json!({
            "index": idx + 1,
            "action": action,
            "code": code,
            "message": clip(msg, if code == 0 { 300 } else { 2000 }),
            "data": data,
            "elapsed_ms": start.elapsed().as_millis() as u64,
        });
        if let Some(pid) = step_args_page(&current) {
            step_result["page_id"] = json!(pid);
        }
        results.push(step_result);
        if code != 0 {
            failed_at = Some(idx + 1);
            first_error = Some(code);
            if stop_on_error {
                break;
            }
        }
    }
    let completed = failed_at.is_none();
    let mut data = json!({
        "mode": "continuous",
        "steps": results,
        "step_count": results.len(),
        "completed": completed,
        "stop_on_error": stop_on_error,
        "page_id": current,
    });
    if !spawned.is_empty() {
        data["spawned_page_id"] = json!(spawned);
    }
    if let Some(i) = failed_at {
        data["failed_at"] = json!(i);
    }
    if completed {
        envelope(0, "sequence completed", data)
    } else {
        envelope(
            first_error.unwrap_or(2002),
            &format!("sequence 未完成:第 {} 步失败", failed_at.unwrap_or(0)),
            data,
        )
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn step_args_page(current: &str) -> Option<String> {
    if current.is_empty() {
        None
    } else {
        Some(current.to_string())
    }
}

async fn run_explore(args: Value) -> Result<String> {
    let Some(queries) = args.get("queries").and_then(Value::as_array).cloned() else {
        return envelope(1001, "explore 需要 queries", json!({}));
    };
    if queries.is_empty() || queries.len() > 8 {
        return envelope(
            1001,
            "queries 数量必须在 1..=8",
            json!({"count": queries.len(), "max": 8}),
        );
    }
    let mut page_id = str_arg(&args, "page_id").unwrap_or("").to_string();
    let mut auto_opened = false;
    if page_id.is_empty() {
        let Some(url) = str_arg(&args, "url") else {
            return envelope(1001, "explore 需要 page_id 或 url", json!({}));
        };
        let mut open_args = json!({"action": "open", "url": url});
        for key in ["timeout_ms", "mode", "reuse", "user_agent"] {
            if let Some(v) = args.get(key) {
                open_args[key] = v.clone();
            }
        }
        let out = run_open(open_args).await?;
        let parsed: Value = serde_json::from_str(&out).unwrap_or(json!({}));
        if parsed.get("code").and_then(Value::as_i64) != Some(0) {
            return Ok(out);
        }
        page_id = parsed
            .pointer("/data/page_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        auto_opened = true;
    } else if !McpWebLsmBrowserUseTool::mgr().page_exists(&page_id).await {
        return envelope(2000, "page_id 无效", json!({"page_id": page_id}));
    }
    let mut results = Vec::new();
    let mut ok_count = 0;
    let mut err_count = 0;
    for q in queries {
        let info = q.get("info").and_then(Value::as_str).unwrap_or("");
        if info.is_empty() {
            results.push(json!({"info": "", "code": 1001, "message": "query 缺少 info", "data": {}}));
            err_count += 1;
            continue;
        }
        let q_params = q.get("params").cloned().unwrap_or(json!({}));
        let out = inspect::run(json!({
            "page_id": page_id,
            "info": info,
            "params": q_params,
        }))
        .await
        .unwrap_or_else(|e| json!({"code": 2001, "message": e.to_string(), "data": {}}).to_string());
        let parsed: Value = serde_json::from_str(&out).unwrap_or(json!({"code": 2002}));
        let code = parsed.get("code").and_then(Value::as_i64).unwrap_or(2002);
        if code == 0 {
            ok_count += 1;
        } else {
            err_count += 1;
        }
        results.push(json!({
            "info": info,
            "code": code,
            "message": parsed.get("message").and_then(Value::as_str).unwrap_or(""),
            "data": parsed.get("data").cloned().unwrap_or(json!({})),
        }));
    }
    let summary = str_arg(&args, "summary_hint")
        .or_else(|| str_arg(&args, "summary"))
        .unwrap_or("");
    let mut data = json!({
        "page_id": page_id,
        "results": results,
        "summary_hint": summary,
        "ok_count": ok_count,
        "err_count": err_count,
        "total": ok_count + err_count,
    });
    if auto_opened {
        data["auto_opened"] = json!(true);
    }
    envelope(0, "ok", data)
}
