//! LsmAgentBrowser 内核单元测试。
//!
//! 使用 `data:` 页面,不依赖外网与任何外部浏览器。

use super::*;

fn html_doc(body: &str) -> String {
    format!("data:text/html,<html><head><title>T</title></head><body>{body}</body></html>")
}

#[tokio::test]
async fn 打开_data_页面并读取标题与正文() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<h1>Hello</h1><p>World</p>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    assert_eq!(out.title, "T");
    assert_eq!(out.status, 200);
    let v = inspect::inspect(&mgr, &out.page_id, "title", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(v["title"], "T");
    let text = inspect::inspect(&mgr, &out.page_id, "elements", &serde_json::json!({"selector": "h1"}))
        .await
        .unwrap();
    assert!(text["text"].as_str().unwrap_or("").contains("Hello"));
    mgr.shutdown().await;
}

#[tokio::test]
async fn 点击链接导航到新页面() {
    let mgr = LsmBrowserManager::new();
    let next = html_doc("<h2>Next</h2>");
    let url = html_doc(&format!("<a id='go' href=\"{next}\">go</a>"));
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let v = api::control(
        &mgr,
        Some(&out.page_id),
        "click",
        &serde_json::json!({"selector": "#go"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(v["clicked"], true);
    let title = inspect::inspect(&mgr, &out.page_id, "title", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(title["title"], "T");
    let h2 = inspect::inspect(&mgr, &out.page_id, "elements", &serde_json::json!({"selector": "h2"}))
        .await
        .unwrap();
    assert!(h2["text"].as_str().unwrap_or("").contains("Next"));
    mgr.shutdown().await;
}

#[tokio::test]
async fn 输入文本写入运行时值() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<input id='q' name='q' value=''>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    api::control(
        &mgr,
        Some(&out.page_id),
        "input_text",
        &serde_json::json!({"selector": "#q", "text": "rust"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    let v = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#q"}),
    )
    .await
    .unwrap();
    assert_eq!(v["value"], "rust");
    mgr.shutdown().await;
}

#[tokio::test]
async fn eval_js_可查询_dom_并产生动作() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<h1 id='h'>Hi</h1><input id='q'>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let v = api::control(
        &mgr,
        Some(&out.page_id),
        "eval_js",
        &serde_json::json!({
            "expression": "document.querySelector('#q').value='abc'; document.querySelector('#h').innerText + '|' + document.querySelector('#q').value"
        }),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(v["result"], "Hi|abc");
    let el = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#q"}),
    )
    .await
    .unwrap();
    assert_eq!(el["value"], "abc");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 截图生成_png_文件() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<h1>Shot</h1>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let dir = std::env::temp_dir().join(format!("lsm_shot_{}", now_ms()));
    let path = dir.join("shot.png");
    let v = mgr
        .screenshot(
            &out.page_id,
            &serde_json::json!({"save_path": path.to_string_lossy(), "full_page": true}),
        )
        .await
        .unwrap();
    assert_eq!(v["format"], "png");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    let _ = std::fs::remove_dir_all(&dir);
    mgr.shutdown().await;
}

#[tokio::test]
async fn close_all_清空内存页面() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<p>x</p>");
    mgr.new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    assert!(mgr.has_browser().await);
    let out = mgr.close_page("all").await;
    assert_eq!(out.closed.len(), 1);
    assert!(!mgr.has_browser().await);
}

#[test]
fn 模式解析兼容三档() {
    assert_eq!(BrowserMode::from_arg("headed"), Some(BrowserMode::Headed));
    assert_eq!(BrowserMode::from_arg("hidden"), Some(BrowserMode::Hidden));
    assert_eq!(BrowserMode::from_arg("new_headless"), Some(BrowserMode::Hidden));
    assert_eq!(BrowserMode::from_arg("bogus"), None);
}
