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
//! 设计见 `docs/MCP_Web_Use/01-设计与解决方案.md`。
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

// ===================== action=open(list/close 同级,轻量内联) =====================

/// 启动/接管浏览器并打开一个页面(原 BrowserNew)。
///
/// 返回 `{page_id,title,final_url,mode,next_steps}`;`next_steps` 是 4 步
/// input_text/click/wait/elements 引导(带 selector_hint),Agent 循环在
/// open 成功后会把 next_steps 注入 LLM 上下文(见 `agent_loop.rs` 引导钩子)。
async fn run_open(args: Value) -> Result<String> {
    let Some(url) = str_arg(&args, "url") else {
        return envelope(1001, "缺少 url", json!({}));
    };
    // 三档 mode(第 74 轮):默认 hidden(纯 CDP 无窗口),解决"误开系统默认浏览器"。
    let mode = match str_arg(&args, "mode") {
        Some("headed") | Some("head") => crate::agent::browser::BrowserMode::Headed,
        Some("new_headless") | Some("old_headless") => crate::agent::browser::BrowserMode::NewHeadless,
        _ => crate::agent::browser::BrowserMode::Hidden,
    };
    let connect = str_arg(&args, "connect_url");
    let ua = str_arg(&args, "user_agent");
    match BrowserManager::global().new_page(url, mode, connect, ua).await {
        Ok((page_id, title, final_url)) => {
            if str_arg(&args, "wait_until") == Some("networkidle") {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
            let mut data = json!({
                "page_id": page_id,
                "title": title,
                "final_url": final_url,
                "mode": match mode {
                    crate::agent::browser::BrowserMode::Hidden => "hidden",
                    crate::agent::browser::BrowserMode::NewHeadless => "new_headless",
                    crate::agent::browser::BrowserMode::Headed => "headed",
                },
                // next_steps —— 分步引导,降低 LLM 编排成本(第 74 轮):
                // input_text → click → wait → elements 四步最常见动作链。
                "next_steps": [
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
                ],
            });
            if let Some(arr) = args.get("block_resources").and_then(Value::as_array) {
                data["block_resources_hint"] = json!(arr);
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
async fn run_close(args: Value) -> Result<String> {
    let Some(id) = str_arg(&args, "page_id") else {
        return envelope(1001, "缺少 page_id", json!({}));
    };
    if BrowserManager::global().close_page(id).await {
        envelope(0, "closed", json!({"page_id":id}))
    } else {
        envelope(2000, "page_id 不存在", json!({"page_id":id}))
    }
}

// ===================== MCP_Web_Use 工具门面 =====================

/// 浏览器网页操控统一入口(MCP 风格单工具 + action 分发)。
pub struct McpWebUseTool;

/// 工具描述:同时承担「使用说明」职责(原 Chromium-WebUse Agent 系统提示词的
/// 作业规范部分精炼,全文见 SubAgent-Work 系统提示词的 MCP_Web_Use 段)。
const MCP_WEB_USE_DESCRIPTION: &str = r#"通过 CDP 驱动 Chromium 系浏览器操作网页(macOS / Windows / Linux,内存无头浏览器默认,也可接管已开浏览器;MCP 风格单工具多 action)。
用 action 参数选择操作:
- open(url*, mode?, connect_url?, user_agent?, wait_until?): 启动/接管 Chromium 并打开页面。默认纯 CDP 嵌入式无头浏览器(mode=hidden,无可见窗口,一次性临时 profile 不干扰日常浏览器);connect_url 接管已用 --remote-debugging-port 启动的浏览器。返回 {page_id,title,final_url,mode,next_steps};next_steps 是 input_text→click→wait→elements 四步引导(selector_hint 已备好)。未检测到浏览器返回 code=3001(确定性失败,如实告知用户安装引导,不要重试)。
- list(): 列出当前存活页面 [{page_id,url,title,created_at}];返回前自动清理失效 entry。冷启动后多轮任务优先用它同步页面索引。
- close(page_id*): 关闭指定页面;最后一个页面关闭时回收浏览器进程。幂等;对话型页面(用户可能继续追问)可保留复用。
- control(page_id*, control_action*, params?): 全部写操作统一入口。control_action 枚举:click/human_click/right_click/double_click/hover/scroll/scroll_to/key_press/press_sequence/input_text/human_input/clear_input/upload_file/select_option/new_tab/close_tab/navigate/back/forward/reload/wait/eval_js/set_cookie/delete_cookie/set_storage/clear_storage/set_viewport/screenshot/heartbeat/drag/focus/blur/mouse_move/dispatch_event。点击链接/new_tab 派生的新标签页经响应 spawned_page_id 回传,后续操作新页面必须用新 page_id。
- inspect(page_id*, info*, params?): 全部只读观察统一入口。info 枚举:console(控制台输出)/network(请求响应流)/elements(元素文本与矩形)/dom(outerHTML 或节点树)/localstorage/sessionstorage/cookies/screenshot/page_meta/viewport/url/title/ping/image_urls。

【标准作业顺序】open 拿 page_id → control 执行动作 → inspect 观察结果 → 任务完成后 close 释放(确定不再需要的页面)。
【错误码对策】1001 修正参数;2000 page_id 失效→action=list 重新同步;2001 断连→重新 open;2002 换 selector 或 input_text 的 use_js 路径重试;3001 未安装浏览器→如实告知用户,不要编造结果。
【作业要点】中文输入优先 params.use_js=true(React/Vue 受控组件兼容);复杂页面先 inspect(info=elements) 探测真实 DOM 再操作,不要硬猜 selector;AI 对话类网站回复等待用 control(wait, selector=[class*=response]..., timeout_ms=60000);截图优先 params.save_path 落盘;DOM 提取注意 truncated 标记分段。
【安全红线】禁止对疑似支付/删除/确认提交类按钮做无把握点击;登录凭证只填用户明确提供的账号密码,不要编造;只读优先——能 inspect 回答的问题不做任何写操作。"#;

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
                    "enum": ["open", "list", "close", "control", "inspect"],
                    "description": "要执行的浏览器操作:open(启动/接管浏览器并打开页面) / list(列出存活页面) / close(关闭页面) / control(写操作统一入口) / inspect(只读观察统一入口)"
                },
                "url": { "type": "string", "description": "open 必填:目标网址" },
                "mode": { "type": "string", "enum": ["hidden", "new_headless", "headed"], "default": "hidden", "description": "open 可选:浏览器模式;hidden=纯 CDP 无窗口(默认,推荐),new_headless=旧 headless=true,headed=可见窗口(调试截图)" },
                "connect_url": { "type": "string", "description": "open 可选:接管已开浏览器,如 http://127.0.0.1:9222(需 --remote-debugging-port 启动)" },
                "user_agent": { "type": "string", "description": "open 可选:覆盖 User-Agent" },
                "wait_until": { "type": "string", "enum": ["load", "domcontentloaded", "networkidle"], "description": "open 可选:打开后额外等待(networkidle 额外等 1.5s)" },
                "block_resources": { "type": "array", "items": { "type": "string" }, "description": "open 可选:拦截资源类型(image/stylesheet/font/media/script),当前仅记录提示" },
                "page_id": { "type": "string", "description": "close/control/inspect 必填:open/list 返回或 spawned_page_id 回传的页面句柄" },
                "control_action": {
                    "type": "string",
                    "enum": [
                        "click", "human_click", "right_click", "double_click", "hover",
                        "scroll", "scroll_to", "key_press", "press_sequence",
                        "input_text", "human_input", "clear_input",
                        "upload_file", "select_option",
                        "new_tab", "close_tab",
                        "navigate", "back", "forward", "reload",
                        "wait", "eval_js",
                        "set_cookie", "delete_cookie",
                        "set_storage", "clear_storage", "set_viewport",
                        "screenshot", "heartbeat",
                        "drag", "focus", "blur", "mouse_move", "dispatch_event"
                    ],
                    "description": "control 必填:具体写操作(鼠标/键盘/输入/上传/标签页/导航/等待/JS/Cookie/Storage/视口/截图等 34 个)"
                },
                "info": {
                    "type": "string",
                    "enum": [
                        "console", "network", "elements", "dom",
                        "localstorage", "sessionstorage", "cookies",
                        "screenshot", "page_meta", "viewport",
                        "url", "title", "ping", "image_urls"
                    ],
                    "description": "inspect 必填:观察维度(Console 输出 / Network 流 / Elements 元素 / DOM / localStorage 等 14 个)"
                },
                "params": { "type": "object", "description": "control/inspect 可选:动作参数对象(selector/text/key/keys/timeout_ms/url/x/y/file_paths/items 等按 control_action/info 各异)" }
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
            other => envelope(1001, "未知 action", json!({"action": other})),
        }
    }
}
