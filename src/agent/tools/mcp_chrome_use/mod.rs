//! MCP_Web_Chrome_Use:Chrome/CDP 浏览器网页操控工具(双浏览器架构的默认优先工具)。
//!
//! 定位:日常 80-90% 的网页打开/读取/交互走本工具(真 Chromium 渲染,
//! headed 窗口支持人工介入);仅当目标站点对 Chrome 风控(验证码/抹除/指纹拦截)
//! 时才切换 `MCP_Web_LsmAgentBrowser_Use`(进程内反爬内核)。
//! 选用决策与阶段路线图见 `docs/双浏览器工具架构/01-设计与解决方案.md`。
//!
//! 对外契约与 Lsm 版对齐:统一 JSON 信封 `{code,message,data}`、page_id 生命周期
//! (`c_` + 8 位十六进制)、`6001` 任务锚点越界语义。错误码:
//! `1001` 参数错 / `2001` 执行失败 / `2004` 页面不存在 / `3001` 未检测到浏览器 /
//! `4030` 工具被 `LAEW_CHROME_USE=off` 关闭 / `6001` 任务锚点越界。
//!
//! 基础框架 action 集:`open` / `list` / `close` / `control`(navigate/click/click_text/
//! type|input_text/key_press/eval_js/screenshot/scroll/wait/back/forward/reload) /
//! `inspect`(url/title/text/html/elements/screenshot)。`inspect(elements)` 是元素
//! 发现原语(可见可交互元素 + 稳定 selector),`type` 未命中回输入框候选,避免
//! 模型盲猜 CSS 选择器。高级动作(HITL/sequence/explore 等)按设计文档分阶段补齐。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Tool;
use crate::agent::chrome_browser::{self, ChromeManager};
use crate::error::Result;

#[cfg(test)]
mod tests;

/// 工具名(LLM 可见的 Chrome 浏览器操控入口,默认优先使用)。
pub const MCP_WEB_CHROME_USE_TOOL_NAME: &str = "MCP_Web_Chrome_Use";

pub(super) const CODE_PARAM_ERROR: i32 = 1001;
pub(super) const CODE_EXEC_FAILED: i32 = 2001;
pub(super) const CODE_PAGE_NOT_FOUND: i32 = 2004;
pub(super) const CODE_NO_BROWSER: i32 = 3001;
pub(super) const CODE_DISABLED: i32 = 4030;
pub(super) const CODE_TARGET_ANCHOR_VIOLATION: i32 = 6001;

/// `LAEW_CHROME_USE=off|0|false|no` 关闭本工具(回退单内核行为)。
pub(crate) fn chrome_use_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_CHROME_USE")
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 统一 JSON 信封(与 Lsm 版逐字段一致)。
pub(super) fn envelope(code: i32, message: &str, data: Value) -> Result<String> {
    Ok(json!({"code": code, "message": message, "data": data}).to_string())
}

/// inspect(text):支持 `params.selector` 限定子树 + 输出上限。
async fn inspect_text(
    page: &chromiumoxide::Page,
    params: &Value,
) -> std::result::Result<Value, (i32, String)> {
    let max_chars = max_chars_arg(params);
    let selector = str_arg(params, "selector");
    let text = match selector {
        Some(sel) => chrome_browser::element_text(page, sel).await,
        None => chrome_browser::visible_text(page).await.map(Some),
    }
    .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))?;
    Ok(match text {
        Some(t) => {
            let (text, chars, truncated) = cap_chars(t, max_chars);
            json!({"text": text, "chars": chars, "truncated": truncated, "selector": selector})
        }
        None => json!({"text": null, "chars": 0, "truncated": false, "selector": selector,
                       "note": "选择器未命中元素"}),
    })
}

/// inspect(html):支持 `params.selector` 限定子树 + 输出上限。
async fn inspect_html(
    page: &chromiumoxide::Page,
    params: &Value,
) -> std::result::Result<Value, (i32, String)> {
    let max_chars = max_chars_arg(params);
    let selector = str_arg(params, "selector");
    let html = match selector {
        Some(sel) => chrome_browser::element_html(page, sel).await,
        None => chrome_browser::outer_html(page).await.map(Some),
    }
    .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))?;
    Ok(match html {
        Some(h) => {
            let (html, chars, truncated) = cap_chars(h, max_chars);
            json!({"html": html, "chars": chars, "truncated": truncated, "selector": selector})
        }
        None => json!({"html": null, "chars": 0, "truncated": false, "selector": selector,
                       "note": "选择器未命中元素"}),
    })
}

/// inspect(elements):列出可见可交互元素,支持 `params.selector` 限定作用域、
/// `params.text` 文本过滤、`params.max` 数量上限(默认 80,夹在 [1,200])。
async fn inspect_elements(
    page: &chromiumoxide::Page,
    params: &Value,
) -> std::result::Result<Value, (i32, String)> {
    chrome_browser::visible_elements(
        page,
        str_arg(params, "selector"),
        str_arg(params, "text"),
        max_elements_arg(params),
    )
    .await
    .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))
}

pub(super) fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

pub(super) fn i64_arg(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

/// 任务锚点:目标站外导航阻断(与 Lsm 版 6001 契约一致)。
pub(super) fn target_anchor_guard(url: &str) -> Option<Result<String>> {
    let v = crate::agent::safety::check_open_against_target_anchor(url)?;
    tracing::warn!(
        requested_host = %v.host,
        allowed_hosts = ?v.allowed_hosts,
        "MCP_Web_Chrome_Use 目标站点越界已阻断(任务锚点,code=6001)"
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

fn disabled() -> Result<String> {
    envelope(
        CODE_DISABLED,
        "MCP_Web_Chrome_Use 已被 LAEW_CHROME_USE=off 关闭",
        json!({ "switch": "LAEW_CHROME_USE" }),
    )
}

/// 取 control/inspect 参数对象(兼容顶层直传)。
pub(super) fn params_of(args: &Value) -> &Value {
    args.get("params").filter(|p| p.is_object()).unwrap_or(args)
}

/// inspect(text|html) 输出上限:默认 20000 字符,夹在 [1000, 200000]。
///
/// 整页 HTML 在重站点常达 40 万字符(Baidu Map 实测约 400KB),直接回灌会
/// 挤爆上下文并拖慢推理;给选择器时可只取目标子树,上限仍兜底。
pub(super) fn max_chars_arg(params: &Value) -> usize {
    params
        .get("max_chars")
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .map(|v| v.clamp(1_000, 200_000) as usize)
        .unwrap_or(20_000)
}

/// 截断到 `max_chars` 个字符(按字符边界,不切坏 UTF-8),返回
/// `(文本, 原始字符数, 是否截断)`。
/// inspect(elements) 数量上限:默认 80,夹在 [1,200]。
pub(super) fn max_elements_arg(params: &Value) -> usize {
    params
        .get("max")
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .map(|v| v.clamp(1, 200) as usize)
        .unwrap_or(80)
}

pub(super) fn cap_chars(s: String, max_chars: usize) -> (String, usize, bool) {
    let chars = s.chars().count();
    if chars <= max_chars {
        return (s, chars, false);
    }
    let cut: String = s.chars().take(max_chars).collect();
    (
        format!("{cut}\n…(截断:原始 {chars} 字符,仅显示前 {max_chars} 字符;可加 params.selector 或调大 params.max_chars)"),
        chars,
        true,
    )
}

/// MCP_Web_Chrome_Use 工具。
pub struct McpChromeUseTool;

impl McpChromeUseTool {
    fn mgr() -> std::sync::Arc<ChromeManager> {
        ChromeManager::global()
    }

    fn page_of(page_id: &str) -> Result<std::sync::Arc<chromiumoxide::Page>> {
        Self::mgr()
            .get_page(page_id)
            .ok_or_else(|| crate::error::AgentError::ToolExecution { tool: "MCP_Web_Chrome_Use".into(), reason: page_missing_message(page_id)})
    }
}

/// 2004 诊断文案:跨工具 page_id 误用给出可操作指引(实测 QC 曾建议把 Chrome
/// 的 c_ page_id 传给 Lsm 工具 explore,确定性陷阱,工具层直接点破)。
fn page_missing_message(page_id: &str) -> String {
    if let Some(rest) = page_id.strip_prefix("p_") {
        if rest.chars().all(|c| c.is_ascii_hexdigit()) {
            return format!(
                "页面不存在:{page_id} 属于 MCP_Web_LsmAgentBrowser_Use 的注册表\
                 (两浏览器工具页面注册表独立,page_id 不跨工具复用)。\
                 请用 action=open 重新打开页面,或 action=list 查看本工具存活页面"
            );
        }
    }
    format!("页面不存在(可能已关闭或从未打开):{page_id};用 action=list 查看本工具存活页面")
}

/// 页面句柄仍在注册表、但底层 target 已销毁/崩溃(Chrome 155 首启偶发)。
/// 原始错误是 futures-channel 的 `send failed because receiver is gone`。
pub(super) fn is_dead_page_error(msg: &str) -> bool {
    msg.contains("receiver is gone")
        || msg.contains("Target closed")
        || msg.contains("Session closed")
        || msg.contains("target closed")
}

fn dead_page_message(page_id: &str, raw: &str) -> String {
    format!(
        "页面已失效(page_id={page_id} 的标签页已被销毁或崩溃):{raw};已从注册表剔除,\
         请用 action=open 重新打开页面后继续"
    )
}

/// 动作失败后若判定为死页,同步剔除注册表条目(避免下次 list 再返回幽灵页)。
fn evict_if_dead(page_id: &str, msg: &str) -> bool {
    if is_dead_page_error(msg) {
        McpChromeUseTool::mgr().close_page(page_id);
        true
    } else {
        false
    }
}

#[async_trait]
impl Tool for McpChromeUseTool {
    fn name(&self) -> &str {
        MCP_WEB_CHROME_USE_TOOL_NAME
    }

    fn description(&self) -> &str {
        "浏览器网页操控工具(Chrome/CDP,真 Chromium 渲染,双浏览器架构中默认优先使用)。\
支持 headed 有头窗口(人工可直接看到并介入页面)与 hidden 无头模式(LAEW_CHROME_MODE)。\
action=open 打开页面拿 page_id;list 列出页面;control 执行写操作(navigate/click/click_text/type|input_text/\
key_press/eval_js/screenshot/scroll/wait/back/forward/reload);inspect 观测页面(url/title/text/html/elements/screenshot);\
close 释放页面(page_id=all 全关并回收 Chrome 进程)。\
★ **找不到选择器时先 inspect(info=\"elements\", params.text=\"路线\")**:列出可见可交互元素\
(每项含可直接回传的 selector/tag/id/class/placeholder/text),再 click/type;不要盲猜 CSS。\
★ 纯文字按钮(「选为终点」「查询」「公交」等)优先 click_text(params.text=可见文字,可加 params.selector 限定作用域);\
只命中 tooltip 时会自动点相邻图标按钮。\
★ type/input_text 选择器未命中会回 candidates(可见输入框 selector/placeholder),按候选改选。\
★ 联想/快捷键/回车提交用 key_press(params.key=Enter|Tab|ArrowDown|Escape...;可带 params.selector 先聚焦),\
它发送真实 CDP 键盘事件;type 只做赋值+input/change。\
★ screenshot 务必给 params.save_path 落盘(只回路径+尺寸);不传才会回 base64。\
★ inspect(html|text) 可用 params.selector 只取目标子树,默认 20000 字符截断(params.max_chars 可调),\
避免整页 HTML 撑爆上下文。\
★ wait 支持 selector 就绪判定(params.selector+timeout_ms)。\
page_id 仅本工具内有效,不跨工具复用。\
当目标站点对 Chrome 风控(反复验证码/页面抹除/访问被拒)时,改用 MCP_Web_LsmAgentBrowser_Use。\
返回统一信封 {code,message,data};6001=任务锚点越界,3001=未检测到浏览器。"
    }

    fn parameters(&self) -> Value {
        // 顶层不设 additionalProperties=false:运行时 `params_of` 明确兼容
        // 「动作参数顶层直传」,模型也常把 max_chars/max/text 放顶层。
        // Schema 拒绝会白白消耗两轮(实测 2026-10-23 复跑)。
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "list", "close", "control", "inspect"]
                },
                "url": { "type": "string", "description": "open 的目标网址;control(navigate) 亦可顶层直传" },
                "page_id": { "type": "string", "description": "close/control/inspect 必填;close 可为 all" },
                "control_action": {
                    "type": "string",
                    "enum": [
                        "navigate", "click", "click_text", "type", "input_text",
                        "key_press", "eval_js", "screenshot", "scroll", "wait",
                        "back", "forward", "reload"
                    ]
                },
                "info": {
                    "type": "string",
                    "enum": ["url", "title", "text", "html", "elements", "screenshot"]
                },
                "params": {
                    "type": "object",
                    "description": "control/inspect 动作参数",
                    "properties": {
                        "url": { "type": "string" },
                        "selector": { "type": "string", "description": "click/click_text/type/input_text/key_press/scroll/wait(CSS);wait 时为就绪判定目标;inspect 时限定取哪个子树" },
                        "text": { "type": "string", "description": "type/input_text 的输入文本;click_text 时为要点击的可见文字" },
                        "exact": { "type": "boolean", "description": "click_text 是否要求文本完全相等(默认 false=包含匹配)" },
                        "key": { "type": "string", "description": "key_press 的键名:Enter/Tab/Escape/ArrowDown/ArrowUp/ArrowLeft/ArrowRight/Backspace/Delete/Space 或单字符" },
                        "max_chars": { "type": "integer", "description": "inspect(text|html) 输出上限(默认 20000,范围 1000-200000)" },
                        "max": { "type": "integer", "description": "inspect(elements) 最多返回多少个元素(默认 80,范围 1-200)" },
                        "script": { "type": "string", "description": "eval_js 的 JS 表达式/语句" },
                        "y": { "type": "integer", "description": "scroll 像素(正向下)" },
                        "ms": { "type": "integer", "description": "wait 固定等待毫秒(上限 30000)" },
                        "timeout_ms": { "type": "integer", "description": "wait(selector=...) 就绪判定超时,默认 10000,上限 30000" },
                        "save_path": { "type": "string", "description": "screenshot 落盘路径(.png);给了就只回路径+尺寸,绝不回 base64(防上下文爆炸)" }
                    }
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if !chrome_use_enabled() {
            return disabled();
        }
        let Some(action) = str_arg(&args, "action").map(str::to_string) else {
            return envelope(CODE_PARAM_ERROR, "缺少 action", json!({}));
        };
        match action.as_str() {
            "open" => run_open(args).await,
            "list" => run_list().await,
            "close" => run_close(args).await,
            "control" => run_control(args).await,
            "inspect" => run_inspect(args).await,
            other => envelope(CODE_PARAM_ERROR, "未知 action", json!({"action": other})),
        }
    }
}

// ===================== open / list / close =====================

async fn run_open(args: Value) -> Result<String> {
    let Some(url) = str_arg(&args, "url").map(str::to_string) else {
        return envelope(CODE_PARAM_ERROR, "open 缺少 url", json!({}));
    };
    if let Some(blocked) = target_anchor_guard(&url) {
        return blocked;
    }
    let mgr = McpChromeUseTool::mgr();
    match mgr.open(&url).await {
        Ok((page_id, final_url, title)) => envelope(
            0,
            "ok",
            json!({
                "page_id": page_id,
                "url": final_url,
                "title": title,
                "engine": "chrome",
                "mode": if mgr.mode().is_headed() { "headed" } else { "hidden" },
                "tool_hint": "page_id 仅 MCP_Web_Chrome_Use 内有效;另一浏览器工具的 page_id 不跨工具复用",
            }),
        ),
        Err(e) => {
            let msg = e.to_string();
            let code = if msg.contains("未检测到") {
                CODE_NO_BROWSER
            } else {
                CODE_EXEC_FAILED
            };
            envelope(code, &msg, json!({"url": url}))
        }
    }
}

async fn run_list() -> Result<String> {
    let pages = McpChromeUseTool::mgr().list_pages().await;
    envelope(
        0,
        "ok",
        json!({ "pages": pages.iter().map(|(id, url)| json!({"page_id": id, "url": url})).collect::<Vec<_>>() }),
    )
}

async fn run_close(args: Value) -> Result<String> {
    let Some(page_id) = str_arg(&args, "page_id").map(str::to_string) else {
        return envelope(CODE_PARAM_ERROR, "close 缺少 page_id", json!({}));
    };
    let mgr = McpChromeUseTool::mgr();
    if page_id == "all" {
        mgr.close_all();
        return envelope(0, "ok", json!({"closed": "all"}));
    }
    if mgr.close_page(&page_id) {
        envelope(0, "ok", json!({"closed": page_id}))
    } else {
        envelope(CODE_PAGE_NOT_FOUND, "页面不存在", json!({"page_id": page_id}))
    }
}

// ===================== control =====================

async fn run_control(args: Value) -> Result<String> {
    let Some(page_id) = str_arg(&args, "page_id").map(str::to_string) else {
        return envelope(CODE_PARAM_ERROR, "control 缺少 page_id", json!({}));
    };
    let Some(control_action) = str_arg(&args, "control_action").map(str::to_string) else {
        return envelope(CODE_PARAM_ERROR, "control 缺少 control_action", json!({}));
    };
    let page = match McpChromeUseTool::page_of(&page_id) {
        Ok(p) => p,
        Err(e) => {
            return envelope(
                CODE_PAGE_NOT_FOUND,
                &e.to_string(),
                json!({"page_id": page_id, "control_action": control_action}),
            )
        }
    };
    let p = params_of(&args);

    let result: std::result::Result<Value, (i32, String)> = match control_action.as_str() {
        "navigate" => {
            let Some(url) = str_arg(p, "url").or_else(|| str_arg(&args, "url")) else {
                return envelope(CODE_PARAM_ERROR, "navigate 缺少 url", json!({}));
            };
            if let Some(blocked) = target_anchor_guard(url) {
                return blocked;
            }
            chrome_browser::navigate(&page, url)
                .await
                .map(|_| json!({"url": url}))
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))
        }
        "click" => {
            let Some(sel) = str_arg(p, "selector") else {
                return envelope(CODE_PARAM_ERROR, "click 缺少 params.selector", json!({}));
            };
            chrome_browser::click(&page, sel)
                .await
                .map(|hit| json!({"selector": sel, "hit": hit}))
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))
        }
        // 按可见文字点击:现代站点纯文本按钮无稳定选择器(「选为终点」)。
        "click_text" => {
            let Some(text) = str_arg(p, "text") else {
                return envelope(CODE_PARAM_ERROR, "click_text 缺少 params.text", json!({}));
            };
            let exact = p.get("exact").and_then(Value::as_bool).unwrap_or(false);
            chrome_browser::click_text(&page, text, str_arg(p, "selector"), exact)
                .await
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))
        }
        // 真实键盘事件:联想/快捷键/Enter 提交走 CDP Input.dispatchKeyEvent。
        "key_press" => {
            let Some(key) = str_arg(p, "key") else {
                return envelope(CODE_PARAM_ERROR, "key_press 缺少 params.key", json!({}));
            };
            chrome_browser::key_press(&page, key, str_arg(p, "selector"))
                .await
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))
        }
        // input_text 与 type 同义(对齐 Lsm 版命名,实测模型对 type 可发现性差,
        // 曾绕行 eval_js 手搓键盘事件)。
        "type" | "input_text" => {
            let (Some(sel), text) = (str_arg(p, "selector"), p.get("text").and_then(Value::as_str).unwrap_or(""))
            else {
                return envelope(CODE_PARAM_ERROR, "type/input_text 缺少 params.selector", json!({}));
            };
            chrome_browser::type_text_ex(&page, sel, text)
                .await
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))
        }
        "eval_js" => {
            let Some(script) = str_arg(p, "script") else {
                return envelope(CODE_PARAM_ERROR, "eval_js 缺少 params.script", json!({}));
            };
            chrome_browser::eval_js(&page, script)
                .await
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))
        }
        // screenshot:优先 save_path 落盘(信封只回元数据,防 base64 爆上下文);
        // 不传 save_path 时保持 base64 内联(兼容旧行为)。
        "screenshot" => match str_arg(p, "save_path") {
            Some(path) => chrome_browser::screenshot_to_file(&page, path)
                .await
                .map(|(path, bytes, w, h)| json!({"format": "png", "path": path, "bytes": bytes, "width": w, "height": h}))
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
            None => chrome_browser::screenshot_base64(&page)
                .await
                .map(|b64| json!({"format": "png", "image_base64": b64}))
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
        },
        "scroll" => {
            let y = i64_arg(p, "y").unwrap_or(0);
            chrome_browser::scroll(&page, y, str_arg(p, "selector"))
                .await
                .map(|_| json!({"scrolled": true}))
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))
        }
        // wait:固定毫秒,或 selector 就绪判定(params.selector + timeout_ms,上限 30s)。
        "wait" => match str_arg(p, "selector") {
            Some(sel) => {
                let timeout_ms = i64_arg(p, "timeout_ms").unwrap_or(10_000).max(0) as u64;
                chrome_browser::wait_for_selector(&page, sel, timeout_ms)
                    .await
                    .map(|hit| json!({"selector": sel, "visible": hit, "timeout_ms": timeout_ms.min(30_000)}))
                    .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))
            }
            None => {
                chrome_browser::wait(i64_arg(p, "ms").unwrap_or(500).max(0) as u64).await;
                Ok(json!({"waited_ms": i64_arg(p, "ms").unwrap_or(500).max(0)}))
            }
        },
        "back" => chrome_browser::eval_js(&page, "history.back(); true")
            .await
            .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
        "forward" => chrome_browser::eval_js(&page, "history.forward(); true")
            .await
            .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
        "reload" => chrome_browser::eval_js(&page, "location.reload(); true")
            .await
            .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
        other => return envelope(CODE_PARAM_ERROR, "未知 control_action(基础框架)", json!({"control_action": other})),
    };

    match result {
        Ok(data) => envelope(0, "ok", json!({"page_id": page_id, "control_action": control_action, "result": data})),
        Err((_, msg)) if evict_if_dead(&page_id, &msg) => envelope(
            CODE_PAGE_NOT_FOUND,
            &dead_page_message(&page_id, &msg),
            json!({"page_id": page_id, "control_action": control_action, "evicted": true}),
        ),
        Err((code, msg)) => envelope(code, &msg, json!({"page_id": page_id, "control_action": control_action})),
    }
}

// ===================== inspect =====================

async fn run_inspect(args: Value) -> Result<String> {
    let Some(page_id) = str_arg(&args, "page_id").map(str::to_string) else {
        return envelope(CODE_PARAM_ERROR, "inspect 缺少 page_id", json!({}));
    };
    let Some(info) = str_arg(&args, "info").map(str::to_string) else {
        return envelope(CODE_PARAM_ERROR, "inspect 缺少 info", json!({}));
    };
    let page = match McpChromeUseTool::page_of(&page_id) {
        Ok(p) => p,
        Err(e) => return envelope(CODE_PAGE_NOT_FOUND, &e.to_string(), json!({"page_id": page_id})),
    };

    let result: std::result::Result<Value, (i32, String)> = match info.as_str() {
        "url" | "title" => {
            let (url, title) = chrome_browser::url_and_title(&page).await;
            Ok(json!({"url": url, "title": title, "info": info}))
        }
        "text" => inspect_text(&page, params_of(&args)).await,
        "html" => inspect_html(&page, params_of(&args)).await,
        "elements" => inspect_elements(&page, params_of(&args)).await,
        "screenshot" => match str_arg(params_of(&args), "save_path") {
            Some(path) => chrome_browser::screenshot_to_file(&page, path)
                .await
                .map(|(path, bytes, w, h)| json!({"format": "png", "path": path, "bytes": bytes, "width": w, "height": h}))
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
            None => chrome_browser::screenshot_base64(&page)
                .await
                .map(|b64| json!({"format": "png", "image_base64": b64}))
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
        },
        other => return envelope(CODE_PARAM_ERROR, "未知 info(基础框架)", json!({"info": other})),
    };

    match result {
        Ok(data) => envelope(0, "ok", json!({"page_id": page_id, "data": data})),
        Err((_, msg)) if evict_if_dead(&page_id, &msg) => envelope(
            CODE_PAGE_NOT_FOUND,
            &dead_page_message(&page_id, &msg),
            json!({"page_id": page_id, "info": info, "evicted": true}),
        ),
        Err((code, msg)) => envelope(code, &msg, json!({"page_id": page_id, "info": info})),
    }
}
