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
    use security::{check_simple_request, CorsHeaders, CorsResult};
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
    // 方法不在白名单 → 拒绝。
    let cors = CorsHeaders {
        allow_origin: Some("*".to_string()),
        allow_methods: Some(vec!["POST".to_string()]),
        ..Default::default()
    };
    assert!(matches!(
        check_simple_request("https://a.com", "GET", &cors, false),
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
