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
//! 基础框架 action 集:`open` / `list` / `close` / `control`(navigate/click/type/
//! eval_js/screenshot/scroll/wait/back/forward/reload) / `inspect`(url/title/text/html/
//! screenshot)。高级动作(HITL/sequence/explore 等)按设计文档分阶段补齐。

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

/// MCP_Web_Chrome_Use 工具。
pub struct McpChromeUseTool;

impl McpChromeUseTool {
    fn mgr() -> std::sync::Arc<ChromeManager> {
        ChromeManager::global()
    }

    fn page_of(page_id: &str) -> Result<std::sync::Arc<chromiumoxide::Page>> {
        Self::mgr()
            .get_page(page_id)
            .ok_or_else(|| crate::error::AgentError::ToolExecution { tool: "MCP_Web_Chrome_Use".into(), reason: format!("page_not_found:{page_id}")})
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
action=open 打开页面拿 page_id;list 列出页面;control 执行写操作(navigate/click/type/\
eval_js/screenshot/scroll/wait/back/forward/reload);inspect 观测页面(url/title/text/html/\
screenshot);close 释放页面(page_id=all 全关并回收 Chrome 进程)。\
当目标站点对 Chrome 风控(反复验证码/页面抹除/访问被拒)时,改用 MCP_Web_LsmAgentBrowser_Use。\
返回统一信封 {code,message,data};6001=任务锚点越界,3001=未检测到浏览器。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
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
                        "navigate", "click", "type", "eval_js",
                        "screenshot", "scroll", "wait",
                        "back", "forward", "reload"
                    ]
                },
                "info": {
                    "type": "string",
                    "enum": ["url", "title", "text", "html", "screenshot"]
                },
                "params": {
                    "type": "object",
                    "description": "control/inspect 动作参数",
                    "properties": {
                        "url": { "type": "string" },
                        "selector": { "type": "string", "description": "click/type/scroll(CSS)" },
                        "text": { "type": "string", "description": "type 的输入文本" },
                        "script": { "type": "string", "description": "eval_js 的 JS 表达式/语句" },
                        "y": { "type": "integer", "description": "scroll 像素(正向下)" },
                        "ms": { "type": "integer", "description": "wait 毫秒(上限 30000)" }
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
        Err(_) => {
            return envelope(
                CODE_PAGE_NOT_FOUND,
                "页面不存在",
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
        "type" => {
            let (Some(sel), text) = (str_arg(p, "selector"), p.get("text").and_then(Value::as_str).unwrap_or(""))
            else {
                return envelope(CODE_PARAM_ERROR, "type 缺少 params.selector", json!({}));
            };
            chrome_browser::type_text(&page, sel, text)
                .await
                .map(|hit| json!({"selector": sel, "hit": hit}))
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
        "screenshot" => chrome_browser::screenshot_base64(&page)
            .await
            .map(|b64| json!({"format": "png", "image_base64": b64}))
            .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
        "scroll" => {
            let y = i64_arg(p, "y").unwrap_or(0);
            chrome_browser::scroll(&page, y, str_arg(p, "selector"))
                .await
                .map(|_| json!({"scrolled": true}))
                .map_err(|e| (CODE_EXEC_FAILED, e.to_string()))
        }
        "wait" => {
            chrome_browser::wait(i64_arg(p, "ms").unwrap_or(500).max(0) as u64).await;
            Ok(json!({"waited_ms": i64_arg(p, "ms").unwrap_or(500).max(0)}))
        }
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
        Err(_) => return envelope(CODE_PAGE_NOT_FOUND, "页面不存在", json!({"page_id": page_id})),
    };

    let result: std::result::Result<Value, (i32, String)> = match info.as_str() {
        "url" | "title" => {
            let (url, title) = chrome_browser::url_and_title(&page).await;
            Ok(json!({"url": url, "title": title, "info": info}))
        }
        "text" => chrome_browser::visible_text(&page)
            .await
            .map(|t| json!({"text": t}))
            .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
        "html" => chrome_browser::outer_html(&page)
            .await
            .map(|h| json!({"html": h}))
            .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
        "screenshot" => chrome_browser::screenshot_base64(&page)
            .await
            .map(|b64| json!({"format": "png", "image_base64": b64}))
            .map_err(|e| (CODE_EXEC_FAILED, e.to_string())),
        other => return envelope(CODE_PARAM_ERROR, "未知 info(基础框架)", json!({"info": other})),
    };

    match result {
        Ok(data) => envelope(0, "ok", json!({"page_id": page_id, "data": data})),
        Err((code, msg)) => envelope(code, &msg, json!({"page_id": page_id, "info": info})),
    }
}
