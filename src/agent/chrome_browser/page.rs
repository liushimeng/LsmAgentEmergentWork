//! Chrome 页面级操作(基础框架):导航 / eval / 截图 / 点击 / 输入 / 滚动 / 等待。
//!
//! 所有 CDP 调用统一走有界墙钟预算(超时即失败返回),根治「导航状态机卡死 →
//! 后续 CDP 消息全部排队 → Agent 循环永久挂起」事故(依据见归档
//! `src/agent/browser_cdp.rs` 头注)。

use std::time::Duration;

use chromiumoxide::Page;
use serde_json::Value;

use crate::error::{AgentError, Result};

/// 单条 CDP 轻量调用的默认墙钟预算(与历史 `CDP_BOUND_SECS` 同值同语义)。
const CDP_BOUND_SECS: u64 = 10;
/// 导航级调用的墙钟预算(load 事件等待,给慢站点留余量)。
const NAV_BOUND_SECS: u64 = 30;

fn err(what: &str, e: impl std::fmt::Display) -> AgentError {
    AgentError::ToolExecution { tool: "chrome_browser".into(), reason: format!("{what} 失败: {e}")}
}

fn timeout_err(what: &str, secs: u64) -> AgentError {
    AgentError::ToolExecution { tool: "chrome_browser".into(), reason: format!("{what} 超时({secs}s),浏览器 handler 可能已卡死")}
}

/// 导航到 URL(goto + 有界等待装载)。
pub async fn navigate(page: &Page, url: &str) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(NAV_BOUND_SECS), page.goto(url))
        .await
        .map_err(|_| timeout_err(&format!("page.goto({url})"), NAV_BOUND_SECS))?
        .map_err(|e| err(&format!("page.goto({url})"), e))?;
    Ok(())
}

/// 读取当前 URL 与标题(探测类,拿不到按空串处理)。
pub async fn url_and_title(page: &Page) -> (String, String) {
    let url = tokio::time::timeout(Duration::from_secs(CDP_BOUND_SECS), page.url())
        .await
        .ok()
        .and_then(|r| r.ok().flatten())
        .unwrap_or_default();
    let title = tokio::time::timeout(Duration::from_secs(CDP_BOUND_SECS), page.get_title())
        .await
        .ok()
        .and_then(|r| r.ok().flatten())
        .unwrap_or_default();
    (url, title)
}

/// 页面级 eval_js:表达式/语句求值,返回 JSON 值(异常/无值按 Null 处理)。
pub async fn eval_js(page: &Page, script: &str) -> Result<Value> {
    let res = tokio::time::timeout(Duration::from_secs(CDP_BOUND_SECS), page.evaluate(script))
        .await
        .map_err(|_| timeout_err("eval_js", CDP_BOUND_SECS))?
        .map_err(|e| err("eval_js", e))?;
    Ok(res.value().cloned().unwrap_or(Value::Null))
}

/// 全页 PNG 截图,返回 base64(与 Lsm 版 `inspect(screenshot)` 信封对齐)。
///
/// CDP 的 `Page.captureScreenshot` 返回的 `data` **本身已是 base64 字符串**,
/// 直接透传(不可再二次编码,否则落盘得到的不是 PNG)。
pub async fn screenshot_base64(page: &Page) -> Result<String> {
    use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotParams;
    let resp = tokio::time::timeout(
        Duration::from_secs(CDP_BOUND_SECS),
        page.execute(CaptureScreenshotParams::builder().build()),
    )
    .await
    .map_err(|_| timeout_err("screenshot", CDP_BOUND_SECS))?
    .map_err(|e| err("screenshot", e))?;
    // `Binary` 是 base64 文本 newtype(其内部 String 即 base64 文本)。
    let b64: &str = AsRef::<str>::as_ref(&resp.result.data);
    Ok(b64.to_string())
}

/// 按 CSS 选择器点击(JS 通道:存在性检查 + click),返回是否命中元素。
pub async fn click(page: &Page, selector: &str) -> Result<bool> {
    let js = format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return false;
            el.scrollIntoView({{block:'center'}});
            el.click();
            return true;
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default()
    );
    let v = eval_js(page, &js).await?;
    Ok(v.as_bool().unwrap_or(false))
}

/// 按 CSS 选择器输入文本(聚焦 + 原生 setter 赋值 + input/change 事件),
/// 返回是否命中元素。
pub async fn type_text(page: &Page, selector: &str, text: &str) -> Result<bool> {
    let js = format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return false;
            el.scrollIntoView({{block:'center'}});
            el.focus();
            var proto = el instanceof HTMLTextAreaElement
                ? HTMLTextAreaElement.prototype
                : HTMLInputElement.prototype;
            var setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
            setter.call(el, {text});
            el.dispatchEvent(new Event('input', {{bubbles:true}}));
            el.dispatchEvent(new Event('change', {{bubbles:true}}));
            return true;
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default(),
        text = serde_json::to_string(text).unwrap_or_default()
    );
    let v = eval_js(page, &js).await?;
    Ok(v.as_bool().unwrap_or(false))
}

/// 滚动:按像素(y 正向下)或滚动到选择器处。
pub async fn scroll(page: &Page, y: i64, selector: Option<&str>) -> Result<()> {
    let js = match selector {
        Some(sel) => format!(
            "(function() {{ var el = document.querySelector({}); if (el) el.scrollIntoView({{block:'center'}}); }})()",
            serde_json::to_string(sel).unwrap_or_default()
        ),
        None => format!("window.scrollBy(0, {y})"),
    };
    eval_js(page, &js).await?;
    Ok(())
}

/// 全页 PNG 截图落盘:返回 `(路径, 字节数, 宽, 高)`。
///
/// 信封只回元数据不回 base64 —— 实测(2026-10-23 双浏览器工具测试)单张 PNG
/// base64 约 1.7MB 直进消息流,30+ 轮后 LLM 调用 >120s 网关超时,
/// Reflection 根因:「截图的大体积二进制回包滞留上下文后,推理失去前进锚点」。
pub async fn screenshot_to_file(page: &Page, path: &str) -> Result<(String, usize, u32, u32)> {
    let b64 = screenshot_base64(page).await?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.trim())
        .map_err(|e| err("screenshot 解码", e))?;
    let (w, h) = png_size(&bytes).unwrap_or((0, 0));
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    std::fs::write(path, &bytes)
        .map_err(|e| err("screenshot 落盘", e))?;
    Ok((path.to_string(), bytes.len(), w, h))
}

/// 从 PNG 字节解析 IHDR 宽高(8B 签名 + 4B 长度 + 4B "IHDR" 之后)。
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((w, h))
}

/// 就绪判定:轮询选择器直到元素可见(上限 timeout_ms,上限 30s)。
/// 返回是否在超时前命中。
pub async fn wait_for_selector(page: &Page, selector: &str, timeout_ms: u64) -> Result<bool> {
    let budget = Duration::from_millis(timeout_ms.min(30_000).max(200));
    let start = std::time::Instant::now();
    let js = format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return false;
            var r = el.getBoundingClientRect();
            var s = getComputedStyle(el);
            return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default()
    );
    loop {
        let v = eval_js(page, &js).await?;
        if v.as_bool().unwrap_or(false) {
            return Ok(true);
        }
        if start.elapsed() >= budget {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 等待固定毫秒(页面 settle 用)。
pub async fn wait(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms.min(30_000))).await;
}

/// 页面可见文本(`body.innerText`,截断由调用方按信封预算处理)。
pub async fn visible_text(page: &Page) -> Result<String> {
    let v = eval_js(page, "document.body ? document.body.innerText : ''").await?;
    Ok(v.as_str().unwrap_or_default().to_string())
}

/// 页面 HTML(`documentElement.outerHTML`)。
pub async fn outer_html(page: &Page) -> Result<String> {
    let v = eval_js(
        page,
        "document.documentElement ? document.documentElement.outerHTML : ''",
    )
    .await?;
    Ok(v.as_str().unwrap_or_default().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 选择器含引号时经 JSON 转义注入,不产生 JS 语法破坏。
    #[test]
    fn click_js_选择器被_json_转义() {
        let sel = "input[name=\"a'b\"]";
        let encoded = serde_json::to_string(sel).unwrap();
        // JSON 转义后:内层双引号变为 \",单引号原样保留。
        assert_eq!(encoded, r#""input[name=\"a'b\"]""#);
        let js = format!("document.querySelector({encoded})");
        assert!(js.contains(&encoded));
        // 绝不允许把未转义的选择器直接拼进 JS 串字面量。
        assert!(!js.contains("document.querySelector(input"));
    }
}
