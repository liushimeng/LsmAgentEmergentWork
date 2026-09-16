//! Chromium-WebUse 工具层:BrowserNew/List/Close/Control/Inspect。
//!
//! 统一 JSON 信封 `{code,message,data}`,让 LLM 可以按错误码机械决策。
//! 当前实现覆盖网页自动化最常用的导航、点击、输入、JS、截图与页面观察;
//! 其余设计清单中的高级 action/info 返回 1001,后续在 CDP 驱动层逐步补齐。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Tool;
use crate::agent::browser::BrowserManager;
use crate::error::Result;

fn envelope(code: i32, message: &str, data: Value) -> Result<String> {
    Ok(json!({"code": code, "message": message, "data": data}).to_string())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

pub struct BrowserNewTool;

#[async_trait]
impl Tool for BrowserNewTool {
    fn name(&self) -> &str {
        "BrowserNew"
    }
    fn description(&self) -> &str {
        "启动/接管 Chromium 并打开一个页面,返回 page_id/title/final_url。支持 url、headless、connect_url。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "url":{"type":"string"},
                "headless":{"type":"boolean","default":true},
                "connect_url":{"type":"string"}
            },
            "required":["url"],
            "additionalProperties":false
        })
    }
    async fn execute(&self, args: Value) -> Result<String> {
        let Some(url) = str_arg(&args, "url") else {
            return envelope(1001, "缺少 url", json!({}));
        };
        let headless = args
            .get("headless")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let connect = str_arg(&args, "connect_url");
        let user_agent = str_arg(&args, "user_agent");
        match BrowserManager::global()
            .new_page(url, headless, connect, user_agent)
            .await
        {
            Ok((page_id, title, final_url)) => envelope(
                0,
                "ok",
                json!({"page_id":page_id,"title":title,"final_url":final_url}),
            ),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("NO_BROWSER") {
                    envelope(3001, &msg, json!({"install":"Chrome / Edge / Chromium"}))
                } else {
                    envelope(2001, &msg, json!({}))
                }
            }
        }
    }
}

pub struct BrowserListTool;
#[async_trait]
impl Tool for BrowserListTool {
    fn name(&self) -> &str {
        "BrowserList"
    }
    fn description(&self) -> &str {
        "列出当前存活的浏览器页面(page_id/url/title/created_at)。"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{},"additionalProperties":false})
    }
    async fn execute(&self, _: Value) -> Result<String> {
        let pages=BrowserManager::global().list_pages().await.into_iter().map(|(page_id,url,title,created_at)|json!({"page_id":page_id,"url":url,"title":title,"created_at":created_at})).collect::<Vec<_>>();
        envelope(0, "ok", json!({"pages":pages}))
    }
}

pub struct BrowserCloseTool;
#[async_trait]
impl Tool for BrowserCloseTool {
    fn name(&self) -> &str {
        "BrowserClose"
    }
    fn description(&self) -> &str {
        "关闭指定 page_id;最后一个页面关闭时回收内部浏览器进程。"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"page_id":{"type":"string"}},"required":["page_id"],"additionalProperties":false})
    }
    async fn execute(&self, args: Value) -> Result<String> {
        let Some(id) = str_arg(&args, "page_id") else {
            return envelope(1001, "缺少 page_id", json!({}));
        };
        if BrowserManager::global().close_page(id).await {
            envelope(0, "closed", json!({"page_id":id}))
        } else {
            envelope(2000, "page_id 不存在", json!({"page_id":id}))
        }
    }
}

pub struct BrowserControlTool;
#[async_trait]
impl Tool for BrowserControlTool {
    fn name(&self) -> &str {
        "BrowserControl"
    }
    fn description(&self) -> &str {
        "对页面执行 click/input_text/navigate/eval_js/screenshot/wait 写操作。"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"page_id":{"type":"string"},"action":{"type":"string","enum":["click","input_text","navigate","eval_js","screenshot","wait"]},"params":{"type":"object"}},"required":["page_id","action"],"additionalProperties":false})
    }
    async fn execute(&self, args: Value) -> Result<String> {
        let Some(id) = str_arg(&args, "page_id") else {
            return envelope(1001, "缺少 page_id", json!({}));
        };
        let Some(action) = str_arg(&args, "action") else {
            return envelope(1001, "缺少 action", json!({}));
        };
        let params = args.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(page) = BrowserManager::global().page(id).await else {
            return envelope(2000, "page_id 不存在", json!({"page_id":id}));
        };
        let result: std::result::Result<Value, String> = match action {
            "click" => {
                let Some(selector) = str_arg(&params, "selector") else {
                    return envelope(1001, "click 缺少 selector", json!({}));
                };
                browser_click(page, selector).await
            }
            "input_text" => {
                let Some(selector) = str_arg(&params, "selector") else {
                    return envelope(1001, "input_text 缺少 selector", json!({}));
                };
                let Some(text) = str_arg(&params, "text") else {
                    return envelope(1001, "input_text 缺少 text", json!({}));
                };
                browser_input(page, selector, text).await
            }
            "navigate" => {
                let Some(url) = str_arg(&params, "url") else {
                    return envelope(1001, "navigate 缺少 url", json!({}));
                };
                browser_navigate(page, url).await
            }
            "eval_js" => {
                let Some(expression) = str_arg(&params, "expression") else {
                    return envelope(1001, "eval_js 缺少 expression", json!({}));
                };
                browser_eval_js(page, expression).await
            }
            "screenshot" => {
                let path = params
                    .get("save_path")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        format!(
                            "/tmp/laew_web_{}.png",
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis())
                                .unwrap_or_default()
                        )
                    });
                browser_screenshot(page, &path).await
            }
            "wait" => {
                let ms = params
                    .get("duration_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(500)
                    .min(10000);
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                Ok(json!({"waited_ms":ms}))
            }
            _ => return envelope(1001, "暂不支持该 action", json!({"action":action})),
        };
        match result {
            Ok(data) => envelope(0, "ok", data),
            Err(e) => envelope(2002, e.as_str(), json!({})),
        }
    }
}

async fn browser_click(
    page: chromiumoxide::Page,
    selector: &str,
) -> std::result::Result<Value, String> {
    page.find_element(selector.to_string())
        .await
        .map_err(|e| e.to_string())?
        .click()
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({"clicked":selector}))
}

async fn browser_input(
    page: chromiumoxide::Page,
    selector: &str,
    text: &str,
) -> std::result::Result<Value, String> {
    page.find_element(selector.to_string())
        .await
        .map_err(|e| e.to_string())?
        .click()
        .await
        .map_err(|e| e.to_string())?
        .type_str(text.to_string())
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({"input":text,"selector":selector}))
}

async fn browser_navigate(
    page: chromiumoxide::Page,
    url: &str,
) -> std::result::Result<Value, String> {
    page.goto(url.to_string())
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({"url":url}))
}

async fn browser_eval_js(
    page: chromiumoxide::Page,
    expression: &str,
) -> std::result::Result<Value, String> {
    let value = page
        .evaluate_expression(expression.to_string())
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({"result":format!("{value:?}")}))
}

async fn browser_screenshot(
    page: chromiumoxide::Page,
    path: &str,
) -> std::result::Result<Value, String> {
    let params = chromiumoxide::page::ScreenshotParams::builder().build();
    let bytes = page.screenshot(params).await.map_err(|e| e.to_string())?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({"save_path":path}))
}

pub struct BrowserInspectTool;
#[async_trait]
impl Tool for BrowserInspectTool {
    fn name(&self) -> &str {
        "BrowserInspect"
    }
    fn description(&self) -> &str {
        "观察页面 page_meta/url/title/content/ping。"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"page_id":{"type":"string"},"info":{"type":"string","enum":["page_meta","url","title","content","ping"]}},"required":["page_id","info"],"additionalProperties":false})
    }
    async fn execute(&self, args: Value) -> Result<String> {
        let Some(id) = str_arg(&args, "page_id") else {
            return envelope(1001, "缺少 page_id", json!({}));
        };
        let Some(info) = str_arg(&args, "info") else {
            return envelope(1001, "缺少 info", json!({}));
        };
        if info == "ping" {
            return envelope(0, "ok", json!({"ok":true,"page_id":id}));
        }
        let Some(page) = BrowserManager::global().page(id).await else {
            return envelope(2000, "page_id 不存在", json!({"page_id":id}));
        };
        let result: std::result::Result<Value, String> = match info {
            "url" => page
                .url()
                .await
                .map(|v| json!({"url":v}))
                .map_err(|e| e.to_string()),
            "title" => page
                .get_title()
                .await
                .map(|v| json!({"title":v}))
                .map_err(|e| e.to_string()),
            "content" => page
                .content()
                .await
                .map(|v| json!({"content":v,"bytes":v.len()}))
                .map_err(|e| e.to_string()),
            "page_meta" => {
                async {
                    let url = page.url().await.map_err(|e| e.to_string())?;
                    let title = page.get_title().await.map_err(|e| e.to_string())?;
                    Ok(json!({"url":url,"title":title}))
                }
                .await
            }
            _ => return envelope(1001, "暂不支持该 info", json!({"info":info})),
        };
        match result {
            Ok(data) => envelope(0, "ok", data),
            Err(e) => envelope(2002, e.as_str(), json!({})),
        }
    }
}
