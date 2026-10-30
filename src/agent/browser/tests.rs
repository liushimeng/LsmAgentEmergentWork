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

#[test]
fn 加载首帧在脚本执行前即可渲染() {
    let mgr = LsmBrowserManager::new();
    let png = mgr
        .loading_view_png("https://a.test/search?a=1&b=<x>", 640, 360)
        .expect("加载首帧应生成 PNG");
    let image = image::load_from_memory(&png)
        .expect("加载首帧应是有效 PNG")
        .to_rgba8();
    assert_eq!((image.width(), image.height()), (640, 360));
    assert!(png.len() > 1_000, "加载首帧不应是空白图: {} bytes", png.len());
}

#[tokio::test]
async fn 正常业务风控词不误报为页面阻断() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<h1>人工智能</h1><a>风控算法</a><p>风控工程师</p>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let v = inspect::inspect(&mgr, &out.page_id, "blockers", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(v["blocked"], false, "正常职位分类不应触发人工介入:{v}");
    assert_eq!(v["blockers"].as_array().map(Vec::len), Some(0), "{v}");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 明确异常访问仍报告页面阻断() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<h1>检测到异常访问,请完成核验</h1>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let v = inspect::inspect(&mgr, &out.page_id, "blockers", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(v["blocked"], true, "明确异常访问仍应阻断:{v}");
    assert_eq!(v["blockers"][0]["kind"], "risk", "{v}");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 页面健康诊断不被webgl申请遮蔽脚本错误() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<h1>静态页头</h1>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    {
        let mut inner = mgr.inner.lock().await;
        let tab = inner.tabs.get_mut(&out.page_id).unwrap();
        tab.webgl_asked = 1;
        tab.console.push(ConsoleRecord {
            level: "error".to_string(),
            text: "公共脚本加载失败".to_string(),
            at_ms: now_ms(),
        });
    }
    let v = inspect::inspect(&mgr, &out.page_id, "page_health", &serde_json::json!({}))
        .await
        .unwrap();
    assert!(v["diagnosis"].as_str().unwrap_or("").contains("脚本执行存在错误"), "{v}");
    assert_eq!(v["console_errors"], 1, "{v}");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 复用页面返回既有有头模式而非硬编码无头() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<h1>P</h1>");
    let first = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    {
        let mut inner = mgr.inner.lock().await;
        inner.tabs.get_mut(&first.page_id).unwrap().mode = BrowserMode::Headed;
    }
    let reused = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    assert!(reused.reused, "第二走同 URL 复用分支:{reused:?}");
    assert_eq!(reused.mode, "headed", "复用结果必须反映既有页面真实模式");
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
async fn 点击select展开面板并点击option选中收起() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<select id='s'><option value='a'>甲</option><option value='b'>乙</option></select>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let opened = api::control(
        &mgr,
        Some(&out.page_id),
        "click",
        &serde_json::json!({"selector": "#s"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(opened["dropdown_open"], true, "点击 select 应展开浮层面板:{opened}");

    let picked = api::control(
        &mgr,
        Some(&out.page_id),
        "click",
        &serde_json::json!({"selector": "#s option:nth-of-type(2)"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(picked["selected"], true, "点击 option 应走选中语义:{picked}");
    assert_eq!(picked["value"], "b");
    assert_eq!(picked["dropdown_closed"], true);

    let selected = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#s"}),
    )
    .await
    .unwrap();
    assert_eq!(selected["value"], "b", "option 点击后运行时值必须同步:{selected}");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 显式select_option后自动收起浮层面板() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<select id='s'><option value='a'>甲</option><option value='b'>乙</option></select>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    api::control(
        &mgr,
        Some(&out.page_id),
        "open_dropdown",
        &serde_json::json!({"selector": "#s"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    api::control(
        &mgr,
        Some(&out.page_id),
        "select_option",
        &serde_json::json!({"selector": "#s", "value": "b"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    let reopened = api::control(
        &mgr,
        Some(&out.page_id),
        "click",
        &serde_json::json!({"selector": "#s"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened["dropdown_open"], true,
        "select_option 后面板应已收起,再次点击 select 应重新展开:{reopened}"
    );
    mgr.shutdown().await;
}

#[tokio::test]
async fn 禁用与只读控件拒绝交互() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<input id='d' disabled><input id='r' readonly><button id='b' disabled>禁用</button>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let clicked = api::control(
        &mgr,
        Some(&out.page_id),
        "click",
        &serde_json::json!({"selector": "#b"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(clicked["clicked"], false, "禁用按钮不得执行点击:{clicked}");
    assert_eq!(clicked["disabled"], true);

    let disabled_input = api::control(
        &mgr,
        Some(&out.page_id),
        "input_text",
        &serde_json::json!({"selector": "#d", "text": "x"}),
        &serde_json::json!({}),
    )
    .await;
    assert!(disabled_input.is_err(), "禁用输入框必须拒绝写入");
    let readonly_input = api::control(
        &mgr,
        Some(&out.page_id),
        "input_text",
        &serde_json::json!({"selector": "#r", "text": "x"}),
        &serde_json::json!({}),
    )
    .await;
    assert!(readonly_input.is_err(), "只读输入框必须拒绝写入");
    mgr.shutdown().await;
}

#[tokio::test]
async fn reset按钮恢复表单默认值() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc(
        "<form id='f'><input id='t' name='t' value='默认'><input id='c' type='checkbox' checked><button id='r' type='reset'>重置</button></form>",
    );
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    // 先改运行时值:输入框写入新值,复选框取消勾选。
    api::control(
        &mgr,
        Some(&out.page_id),
        "input_text",
        &serde_json::json!({"selector": "#t", "text": "改过"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    api::control(
        &mgr,
        Some(&out.page_id),
        "click",
        &serde_json::json!({"selector": "#c"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    let before = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#t"}),
    )
    .await
    .unwrap();
    assert!(
        before["value"].as_str().unwrap_or("").contains("改过"),
        "输入前应写入运行时值:{before}"
    );

    let reset = api::control(
        &mgr,
        Some(&out.page_id),
        "click",
        &serde_json::json!({"selector": "#r"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(reset["form_reset"], true, "reset 应执行本地重置:{reset}");

    let after = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#t"}),
    )
    .await
    .unwrap();
    assert_eq!(
        after["value"].as_str().unwrap_or(""),
        "默认",
        "重置后应恢复 HTML 默认值:{after}"
    );
    let cb = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#c"}),
    )
    .await
    .unwrap();
    assert_eq!(
        cb["state"]["checked"], true,
        "重置后复选框应回到默认勾选:{cb}"
    );
    mgr.shutdown().await;
}

#[tokio::test]
async fn Esc关闭顶层dialog() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<dialog id='d' open>内容</dialog>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let closed = api::control(
        &mgr,
        Some(&out.page_id),
        "key_press",
        &serde_json::json!({"key": "Escape"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(closed["closed"], true, "Esc 应关闭顶层 dialog:{closed}");
    let el = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#d"}),
    )
    .await
    .unwrap();
    assert_eq!(
        el["state"]["open"], false,
        "关闭后 dialog 不应再处于 open:{el}"
    );
    mgr.shutdown().await;
}

#[tokio::test]
async fn 元素内滚动按偏移移动内容() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc(
        "<div id='s' style='overflow:auto;height:60px;width:300px'><p id='p1'>第一段</p><p id='p2'>第二段</p><p id='p3'>第三段</p><p id='p4'>第四段</p></div>",
    );
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let before = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#p1"}),
    )
    .await
    .unwrap();
    let y_before = before["rect"]["y"].as_f64().unwrap_or_default();

    let scrolled = api::control(
        &mgr,
        Some(&out.page_id),
        "scroll",
        &serde_json::json!({"selector": "#s", "delta_y": 30}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(scrolled["mode"], "element_scroll", "{scrolled}");
    assert_eq!(scrolled["scrollTop"], 30.0, "{scrolled}");
    assert!(
        scrolled["maxScroll"].as_f64().unwrap_or(0.0) > 0.0,
        "可滚动容器应有最大滚动量:{scrolled}"
    );

    let after = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#p1"}),
    )
    .await
    .unwrap();
    let y_after = after["rect"]["y"].as_f64().unwrap_or_default();
    assert!(
        (y_before - y_after - 30.0).abs() < 1.0,
        "内容应按滚动偏移上移:before={y_before} after={y_after}"
    );
    mgr.shutdown().await;
}

#[tokio::test]
async fn 点击后active释放且hover命中祖先链() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<div id='parent'><button id='child'>按钮</button></div>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    api::control(
        &mgr,
        Some(&out.page_id),
        "click",
        &serde_json::json!({"selector": "#child"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    let child = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#child"}),
    )
    .await
    .unwrap();
    assert_eq!(child["state"]["hovered"], true, "点击后应保留 hover:{child}");
    assert_eq!(child["state"]["active"], false, "动作完成后 active 必须释放:{child}");

    let parent = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#parent"}),
    )
    .await
    .unwrap();
    assert_eq!(parent["state"]["hovered"], true, "hover 必须命中祖先链:{parent}");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 上传后文件名立即可见() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<input id='f' type='file'>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    api::control(
        &mgr,
        Some(&out.page_id),
        "upload_file",
        &serde_json::json!({"selector": "#f", "file_paths": ["/tmp/报告.pdf"]}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    let el = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "#f"}),
    )
    .await
    .unwrap();
    assert_eq!(el["value"], "/tmp/报告.pdf", "上传路径应进入运行时值:{el}");
    let text = inspect::inspect(&mgr, &out.page_id, "text", &serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        text["text"].as_str().unwrap_or("").contains("报告.pdf"),
        "上传后布局文本应立即显示文件名:{text}"
    );
    mgr.shutdown().await;
}

#[tokio::test]
async fn button型按钮不触发表单提交() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<form action='/search'><input name='q' value='x'><button id='b' type='button'>普通</button></form>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let clicked = api::control(
        &mgr,
        Some(&out.page_id),
        "click",
        &serde_json::json!({"selector": "#b"}),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(clicked["clicked"], true);
    assert!(
        clicked.get("final_url").is_none(),
        "type=button 不得触发表单导航:{clicked}"
    );
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

#[tokio::test]
async fn clip_截图裁剪区域() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<h1>ClipTest</h1>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let dir = std::env::temp_dir().join(format!("lsm_clip_{}", now_ms()));
    let path = dir.join("clip.png");
    // clip 超出整页宽高时应 clamp 到页面实际范围。
    let v = mgr
        .screenshot(
            &out.page_id,
            &serde_json::json!({
                "save_path": path.to_string_lossy(),
                "full_page": true,
                "clip": {"x": 0.0, "y": 0.0, "width": 4096.0, "height": 3000.0},
            }),
        )
        .await
        .unwrap();
    assert_eq!(v["format"], "png");
    assert!(v["clip"].is_object(), "响应应回显生效 clip");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    // 裁剪后尺寸不应超过 clip 请求值。
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert!(decoded.width() <= 4096);
    assert!(decoded.height() <= 3000);
    let _ = std::fs::remove_dir_all(&dir);
    mgr.shutdown().await;
}

#[test]
fn csp_脚本门禁判定() {
    use security::CspPolicy;
    // script-src 'self'：内联被拒（'self' 不含内联语义，需显式 unsafe-inline）。
    let p = CspPolicy::parse("script-src 'self'");
    assert!(!p.is_script_allowed("inline", true), "内联应被拒");
    // 显式 host 来源：外链白名单放行。
    assert!(p.is_script_allowed("https://same.com/a.js", false) == false);
    let p_host = CspPolicy::parse("script-src 'self' https://same.com");
    assert!(p_host.is_script_allowed("https://same.com/a.js", false));
    assert!(!p_host.is_script_allowed("https://other.com/a.js", false));
    // unsafe-inline：内联放行。
    let p2 = CspPolicy::parse("script-src 'self' 'unsafe-inline'");
    assert!(p2.is_script_allowed("inline", true));
    // 无策略：全部放行。
    let p3 = CspPolicy::parse("");
    assert!(p3.is_script_allowed("inline", true));
    assert!(p3.is_script_allowed("https://any.com/a.js", false));
    // connect-src 门禁。
    let p4 = CspPolicy::parse("connect-src https://api.com");
    assert!(p4.is_connect_allowed("https://api.com/data"));
    assert!(!p4.is_connect_allowed("https://evil.com/data"));
}

#[test]
fn cors_跨域校验判定() {
    use security::{check_preflight_request, check_simple_request, CorsHeaders, CorsResult};
    // 无 CORS 头 → 拒绝。
    let cors = CorsHeaders::default();
    assert!(matches!(
        check_simple_request("https://a.com", "GET", &cors, false),
        CorsResult::Denied(_)
    ));
    // 通配 origin + 方法 → 放行。
    let cors = CorsHeaders {
        allow_origin: Some("*".to_string()),
        allow_methods: Some(vec!["GET".to_string()]),
        ..Default::default()
    };
    assert!(matches!(
        check_simple_request("https://a.com", "GET", &cors, false),
        CorsResult::Allowed
    ));
    // 第 178 轮语义修正:**实际响应**只校验 origin —— 浏览器只在预检
    // (OPTIONS)响应里查 Access-Control-Allow-Methods,实际响应本就不带
    // 该头。此前这里同时查 method,所有跨源 API POST 被内核误杀
    // (实测猎聘 api-c 全家 200+空体,职位列表因此全空)。
    let cors = CorsHeaders {
        allow_origin: Some("*".to_string()),
        allow_methods: Some(vec!["POST".to_string()]),
        ..Default::default()
    };
    assert!(matches!(
        check_simple_request("https://a.com", "GET", &cors, false),
        CorsResult::Allowed
    ));
    // 方法白名单仍在**预检**里完整校验。
    assert!(matches!(
        check_preflight_request("https://a.com", "GET", &[], &cors, false),
        CorsResult::Denied(_)
    ));
    // 同源 → 跳过校验。
    assert!(matches!(
        check_simple_request("https://a.com", "GET", &CorsHeaders::default(), true),
        CorsResult::SameOrigin
    ));
}

// ===================== inspect 诊断能力(第 174 轮) =====================

#[tokio::test]
async fn 页面文本与健康度可观测() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<h1>标题文字</h1><p>正文段落</p>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let text = inspect::inspect(&mgr, &out.page_id, "text", &serde_json::json!({}))
        .await
        .unwrap();
    let body = text["text"].as_str().unwrap_or("");
    assert!(body.contains("标题文字"), "text={body}");
    assert!(body.contains("正文段落"), "text={body}");
    assert_eq!(text["source"], "layout");

    let health = inspect::inspect(&mgr, &out.page_id, "page_health", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(health["blank_page"], false);
    assert!(health["visible_text_chars"].as_u64().unwrap_or(0) > 0, "{health}");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 文本极少时_inspect_text_列出控件可访问名() {
    // 画布/图标型 Web App(百度地图)整页 DOM 文本仅 4 字,控件全靠
    // data-title + CSS 背景图呈现 —— `inspect(text)` 在文本极少时补一份
    // 「控件可访问名清单」,Agent 才知道页面上有什么可点的东西。
    let mgr = LsmBrowserManager::new();
    let url = html_doc(
        "<div class='searchbox-content-button route-button' data-title='路线'></div>\
         <input id=sole-input placeholder='搜地点、查公交、找路线'>",
    );
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let text = inspect::inspect(&mgr, &out.page_id, "text", &serde_json::json!({}))
        .await
        .unwrap();
    let controls = text["controls"].as_array().cloned().unwrap_or_default();
    let labels: Vec<String> = controls
        .iter()
        .filter_map(|c| c["label"].as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        labels.iter().any(|l| l == "路线"),
        "应列出 data-title 推出的「路线」:{labels:?}"
    );
    assert!(
        labels.iter().any(|l| l.contains("搜地点")),
        "应列出 placeholder:{labels:?}"
    );
    mgr.shutdown().await;
}

#[tokio::test]
async fn 空白页健康度给出可执行结论() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let health = inspect::inspect(&mgr, &out.page_id, "page_health", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(health["blank_page"], true);
    assert!(health["diagnosis"].as_str().unwrap_or("").len() > 4, "{health}");
    assert!(health["hint"].as_str().unwrap_or("").len() > 4, "{health}");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 被祖先隐藏的元素不再自报可见() {
    // 此前 visible 只看元素自身样式:祖先 display:none 时仍报 visible=true
    // 却给不出矩形,Agent 会把隐藏控件当成可交互目标。
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<div style='display:none'><input id='box'></div>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let v = inspect::inspect(&mgr, &out.page_id, "elements", &serde_json::json!({"selector": "#box"}))
        .await
        .unwrap();
    assert_eq!(v["visible"], false, "{v}");
    assert_eq!(v["rect"]["laid_out"], false, "{v}");
    assert_eq!(v["render_blocked_by"], "ancestor_display_none", "{v}");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 行内元素矩形不再恒为零() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<div><input id='box' style='width:200px;height:30px'></div>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let v = inspect::inspect(&mgr, &out.page_id, "elements", &serde_json::json!({"selector": "#box"}))
        .await
        .unwrap();
    assert_eq!(v["visible"], true, "{v}");
    assert_eq!(v["rect"]["laid_out"], true, "{v}");
    assert_eq!(v["rect"]["width"], 200.0, "{v}");
    assert_eq!(v["rect"]["height"], 30.0, "{v}");
    mgr.shutdown().await;
}

#[tokio::test]
async fn 截图尊重_output_path_别名() {
    use serde_json::json;
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<p>截图</p>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let dir = std::env::temp_dir().join(format!("laew_shot_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let target = dir.join("shot.png");
    let v = api::control(
        &mgr,
        Some(&out.page_id),
        "screenshot",
        &json!({"output_path": target.to_string_lossy()}),
        &json!({}),
    )
    .await
    .unwrap();
    let saved = v["save_path"].as_str().unwrap_or("");
    assert_eq!(saved, target.to_string_lossy(), "{v}");
    assert!(target.exists(), "截图应落到指定路径:{saved}");
    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_dir(&dir);
    mgr.shutdown().await;
}

#[tokio::test]
async fn extract_未声明字段时自动抽取文本() {
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<ul><li>甲条目</li><li>乙条目</li></ul>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let v = inspect::inspect(
        &mgr,
        &out.page_id,
        "extract",
        &serde_json::json!({"item_selector": "li"}),
    )
    .await
    .unwrap();
    assert_eq!(v["auto_fields"], true, "{v}");
    let items = v["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(items.len(), 2, "{v}");
    // 此前未声明 fields 时每条 item 都是空对象 {}。
    assert!(items[0]["text"].as_str().unwrap_or("").contains("甲条目"), "{v}");
    mgr.shutdown().await;
}

// ---------- 有头浏览器窗口交互(2026-10-22 第 174 轮) ----------

#[tokio::test]
async fn 工具栏导航命令_驱动引擎换页() {
    // 浏览器窗口地址栏/工具栏的命令(view::ViewCommand)落到引擎侧:
    // navigate 走真实导航,后退/前进走历史栈。
    let mgr = LsmBrowserManager::new_arc();
    let first = html_doc("<h1>PAGE-1</h1>");
    let second = html_doc("<h1>PAGE-2</h1>");
    let out = mgr
        .new_page(&first, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let pid = out.page_id.clone();

    mgr.handle_view_command(view::ViewCommand::Navigate(second.clone()))
        .await;
    let after = mgr.page_summary(&pid).await.unwrap();
    assert_eq!(after.url, second, "地址栏导航未生效");

    // 后退 → 回到第一页;前进 → 再回第二页。
    mgr.handle_view_command(view::ViewCommand::Back).await;
    let back = mgr.page_summary(&pid).await.unwrap();
    assert_eq!(back.url, first, "后退未生效");
    mgr.handle_view_command(view::ViewCommand::Forward).await;
    let fwd = mgr.page_summary(&pid).await.unwrap();
    assert_eq!(fwd.url, second, "前进未生效");
    mgr.shutdown().await;
}

#[tokio::test]
async fn canvas_区域_渲染出显式占位说明() {
    // 内核未实现 2D/WebGL 像素渲染:画布区域必须显式说明,而不是留一块空白色块。
    let mgr = LsmBrowserManager::new();
    let html = "<html><head><title>C</title></head><body style='margin:0'>\
               <canvas id='c' width='600' height='400' style='display:block;width:600px;height:400px'></canvas>\
               </body></html>";
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(html.as_bytes());
    let url = format!("data:text/html;base64,{b64}");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, Some(800), Some(500), false, true, 10_000)
        .await
        .unwrap();
    let shot = mgr
        .screenshot(&out.page_id, &serde_json::json!({"save_path": "/tmp/laew_test_canvas.png"}))
        .await
        .unwrap();
    assert!(shot["byte_size"].as_u64().unwrap_or(0) > 0);
    // 画布区域应有可见盒(占位底色 + 虚线边框),不是纯白。
    let canvas_box = {
        let inner = mgr.inner.lock().await;
        let tab = inner.tabs.get(&out.page_id).unwrap();
        tab.layout
            .boxes
            .iter()
            .find(|b| b.tag.eq_ignore_ascii_case("canvas") && b.visible)
            .cloned()
    };
    let Some(b) = canvas_box else {
        panic!("canvas 未参与布局");
    };
    assert!(b.w > 100.0 && b.h > 100.0, "canvas 盒尺寸异常:{b:?}");
    mgr.shutdown().await;
}

#[test]
fn page_not_found_对_chrome_page_id_给出跨工具诊断() {
    let e = EngineError::page_not_found("c_00000000");
    assert_eq!(e.code, 2000);
    assert!(
        e.message.contains("MCP_Web_Chrome_Use"),
        "应点破跨工具误用: {}",
        e.message
    );
    assert!(e.message.contains("不跨工具"), "msg={}", e.message);
    // 本工具自己的未知 page_id 保持原通用文案。
    let e2 = EngineError::page_not_found("p_deadbeef");
    assert_eq!(e2.message, "page_id 无效或页面已关闭");
}

#[tokio::test]
async fn 截图ocr参数不吞掉落盘() {
    // `ocr:true` 是「顺带给我文字」,不是「只给文字」。旧实现在此分支提前 return,
    // PNG 永不写盘却返回 code:0 —— Agent 以为落盘成功,靠 Bash ls 才发现文件不存在
    // (实测百度地图任务 wf-2 即被此坑判死)。必须既落盘又回 OCR 文本。
    let mgr = LsmBrowserManager::new();
    let html = "<html><head><title>O</title></head><body style='margin:0'>\
               <p style='font-size:20px'>hello ocr</p></body></html>";
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(html.as_bytes());
    let url = format!("data:text/html;base64,{b64}");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, Some(800), Some(500), false, true, 10_000)
        .await
        .unwrap();
    let path = std::env::temp_dir().join(format!("laew_ocr_test_{}.png", std::process::id()));
    let shot = mgr
        .screenshot(
            &out.page_id,
            &serde_json::json!({"save_path": path.to_string_lossy(), "ocr": true}),
        )
        .await
        .unwrap();
    // 1) 文件必须真的存在且非空
    let bytes = std::fs::read(&path).unwrap_or_else(|_| panic!("ocr=true 时截图文件必须落盘:{path:?}"));
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "应为 PNG:{path:?}");
    assert!(bytes.len() > 500, "PNG 不应为空壳:{}B", bytes.len());
    let _ = std::fs::remove_file(&path);
    // 2) 返回里落盘信息与 OCR 文本并存
    assert!(shot["save_path"].as_str().is_some(), "应回 save_path:{shot}");
    assert!(shot["byte_size"].as_u64().unwrap_or(0) > 0, "应回 byte_size:{shot}");
    assert!(
        shot["ocr_text"].as_str().is_some(),
        "应回 ocr_text(两件事都做):{shot}"
    );
    mgr.shutdown().await;
}

// ---------- 视口自适应(查看器人工拖动窗口 → 宿主重排重渲染) ----------

#[test]
fn 默认视口为1920乘1080() {
    assert_eq!(DEFAULT_VIEWPORT_W, 1920, "默认视口宽应提到 1920");
    assert_eq!(DEFAULT_VIEWPORT_H, 1080, "默认视口高应提到 1080");
    assert_eq!(
        Viewport::default().width,
        1920,
        "Viewport::default 宽应跟随默认视口"
    );
    assert_eq!(
        Viewport::default().height,
        1080,
        "Viewport::default 高应跟随默认视口"
    );
}

#[tokio::test]
async fn 查看器上报尺寸_更新前台页视口() {
    // 人工拖动有头窗口 → 查看器上报 `{"cmd":"resize"}` → 宿主更新视口并重排。
    let mgr = LsmBrowserManager::new_arc();
    let url = html_doc("<h1>P</h1>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, Some(800), Some(600), false, true, 5_000)
        .await
        .unwrap();
    let pid = out.page_id.clone();
    // 置为 headed(不真的拉起查看器:无查看器时 push_frame 是 no-op)。
    {
        let mut inner = mgr.inner.lock().await;
        inner.tabs.get_mut(&pid).unwrap().mode = BrowserMode::Headed;
    }

    mgr.handle_view_command(view::ViewCommand::Resize {
        width: 1280,
        height: 720,
    })
    .await;

    let inner = mgr.inner.lock().await;
    let tab = inner.tabs.get(&pid).unwrap();
    assert_eq!(
        (tab.viewport.width, tab.viewport.height),
        (1280, 720),
        "查看器上报的尺寸应成为新的视口尺寸"
    );
    drop(inner);
    mgr.shutdown().await;
}

#[tokio::test]
async fn 查看器上报尺寸_越界按宿主视口范围夹紧() {
    let mgr = LsmBrowserManager::new_arc();
    let url = html_doc("<h1>P</h1>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, Some(800), Some(600), false, true, 5_000)
        .await
        .unwrap();
    let pid = out.page_id.clone();
    {
        let mut inner = mgr.inner.lock().await;
        inner.tabs.get_mut(&pid).unwrap().mode = BrowserMode::Headed;
    }

    mgr.handle_view_command(view::ViewCommand::Resize {
        width: 99999,
        height: 99999,
    })
    .await;

    let inner = mgr.inner.lock().await;
    let tab = inner.tabs.get(&pid).unwrap();
    assert_eq!(
        (tab.viewport.width, tab.viewport.height),
        (7680, 4320),
        "超范围尺寸应被夹到宿主视口上限"
    );
    drop(inner);
    mgr.shutdown().await;
}

#[tokio::test]
async fn 查看器上报尺寸_非有头页忽略() {
    // 没有查看器的页面(纯无头)不应被 resize 命令改动视口。
    let mgr = LsmBrowserManager::new_arc();
    let url = html_doc("<h1>P</h1>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, Some(800), Some(600), false, true, 5_000)
        .await
        .unwrap();
    let pid = out.page_id.clone();

    mgr.handle_view_command(view::ViewCommand::Resize {
        width: 1280,
        height: 720,
    })
    .await;

    let inner = mgr.inner.lock().await;
    let tab = inner.tabs.get(&pid).unwrap();
    assert_eq!(
        (tab.viewport.width, tab.viewport.height),
        (800, 600),
        "非 headed 页应忽略 resize 命令"
    );
    drop(inner);
    mgr.shutdown().await;
}

// ===================== 第 175 轮:非 HTML 主文档合成展示页 =====================

/// image/* 主文档应合成居中展示页(data: 内嵌,避免二次取回变样);
/// HTML/空 Content-Type 返回 None 走原解析路径。
#[test]
fn 非html文档合成展示页() {
    // 1×1 红色 PNG。
    let png: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R',
        0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0, 0x90, 0x77, 0x53, 0xde, 0, 0, 0, 0x0c, b'I',
        b'D', b'A', b'T', 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x05,
        0x02, 0x00, 0xfe, 0x02, 0x8f, 0xa7, 0x9d, 0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae,
        0x42, 0x60, 0x82,
    ];
    let img = synthesize_non_html_document("image/png", png, "https://q.test/qr?sign=abc")
        .expect("image/* 应合成展示页");
    assert!(img.contains("data:image/png;base64,"), "应 data: 内嵌图片字节");
    assert!(!img.contains("https://q.test/qr?sign=abc\" alt"), "不应外链原图二次取回");
    assert!(img.contains("object-fit"), "应约束展示尺寸");

    let txt = synthesize_non_html_document("text/plain; charset=utf-8", b"hello <world>", "https://a.test/x.txt")
        .expect("text/plain 应合成展示页");
    assert!(txt.contains("&lt;world&gt;"), "文本应 HTML 转义");

    let js = synthesize_non_html_document("application/json", b"{\"a\":1}", "https://a.test/x.json")
        .expect("json 应合成展示页");
    assert!(js.contains("{&quot;a&quot;:1}") || js.contains("{\"a\":1}") || js.contains("&quot;a&quot;"), "json 主体应可见");

    assert!(
        synthesize_non_html_document("text/html; charset=utf-8", b"<html></html>", "https://a.test/").is_none(),
        "HTML 应走原路径"
    );
    assert!(
        synthesize_non_html_document("", b"anything", "https://a.test/").is_none(),
        "空 Content-Type 应走原路径(兼容缺头服务器)"
    );
}

#[tokio::test]
async fn inspect_html_返回整页源码() {
    // 工具描述一直宣传 inspect(info=html),但枚举此前没有它 —— 模型按描述
    // 调用会被参数校验拒绝(实测猎聘任务白费三轮 LLM 往返)。
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<h1>Hello</h1>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let v = inspect::inspect(&mgr, &out.page_id, "html", &serde_json::json!({}))
        .await
        .unwrap();
    let html = v["html"].as_str().unwrap_or("");
    assert!(html.contains("Hello"), "应返回整页 HTML:{html}");
    assert_eq!(v["ok"], true);
    mgr.shutdown().await;
}

#[tokio::test]
async fn inspect_elements_列表模式返回全部匹配项() {
    // 此前 elements 只回 nth 命中的**单个**元素 + count,模型想看「有哪些
    // 匹配项」只能一轮一个元素地试(实测猎聘任务连续多轮误判「弹层不存在」)。
    let mgr = LsmBrowserManager::new();
    let url = html_doc("<a class='x'>A</a><a class='x'>B</a><a class='x'>C</a>");
    let out = mgr
        .new_page(&url, BrowserMode::Hidden, None, None, None, false, true, 5_000)
        .await
        .unwrap();
    let v = inspect::inspect(
        &mgr,
        &out.page_id,
        "elements",
        &serde_json::json!({"selector": "a.x", "list": true}),
    )
    .await
    .unwrap();
    assert_eq!(v["mode"], "list");
    assert_eq!(v["count"], 3);
    assert_eq!(v["items"].as_array().map(|a| a.len()), Some(3));
    let first_text = v["items"][0]["text"].as_str().unwrap_or("");
    assert!(first_text.contains('A'), "首项文本:{first_text}");
    mgr.shutdown().await;
}
