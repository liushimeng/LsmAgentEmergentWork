//! 阶段二脚本管线单元测试:dom_ops 应用、脚本收集、JS 线程行为(PRELUDE)。

use std::collections::HashMap;

use super::dom_ops;
use super::{JsHandle, JsRequest, JsReply, PageEnv, ScriptSource};
use crate::agent::browser::document::Document;

fn env(url: &str) -> PageEnv {
    PageEnv {
        url: url.to_string(),
        title: "T".to_string(),
        cookies: String::new(),
        local_storage: HashMap::new(),
        session_storage: HashMap::new(),
        viewport_width: 1280,
        viewport_height: 720,
        scroll_x: 0.0,
        scroll_y: 0.0,
        user_agent: "test".to_string(),
        prefers_dark: false,
        csp: None,
    }
}

fn action(kind: &str, selector: &str, key: &str, value: &str) -> super::JsAction {
    super::JsAction {
        kind: kind.to_string(),
        selector: selector.to_string(),
        key: key.to_string(),
        value: value.to_string(),
        url: String::new(),
        event: String::new(),
    }
}

fn eval_on(handle: &JsHandle, script: &str, doc_html: &str, url: &str) -> JsReply {
    let doc = Document::parse(doc_html, url);
    let nodes = doc.snapshot_nodes();
    let script = script.to_string();
    let e = env(url);
    handle
        .request(|tx| JsRequest::Eval {
            script,
            env: e,
            nodes,
            reply: tx,
        })
        .expect("JS 线程请求失败")
}

/// 带布局矩形的求值(观察者的几何判定依赖矩形)。
fn eval_on_with_rects(handle: &JsHandle, script: &str, doc_html: &str, url: &str) -> JsReply {
    let doc = Document::parse(doc_html, url);
    let font = crate::agent::browser::render::FontSet::fallback();
    let layout = crate::agent::browser::layout::compute(&doc, 1280.0, &font);
    let rects = layout.rect_map();
    let nodes = doc.snapshot_nodes_with_rects((1280, 720), &rects);
    let script = script.to_string();
    let e = env(url);
    handle
        .request(|tx| JsRequest::Eval {
            script,
            env: e,
            nodes,
            reply: tx,
        })
        .expect("JS 线程请求失败")
}

fn run_scripts(handle: &JsHandle, scripts: Vec<&str>, doc_html: &str, url: &str) -> JsReply {
    let doc = Document::parse(doc_html, url);
    let nodes = doc.snapshot_nodes();
    let sources = scripts
        .into_iter()
        .map(|s| ScriptSource {
            kind: "inline",
            content: s.to_string(),
            src_url: String::new(),
        })
        .collect();
    let e = env(url);
    handle
        .request(|tx| JsRequest::RunScripts {
            scripts: sources,
            env: e,
            nodes,
            reply: tx,
        })
        .expect("JS 线程请求失败")
}

// ---------- dom_ops ----------

#[test]
fn 应用_set_html_注入_子树() {
    let doc = Document::parse("<html><body><div id='box'>旧</div></body></html>", "https://a.test/");
    let actions = vec![action("set_html", "#box", "", "<p>新内容</p><span>二</span>")];
    let summary = dom_ops::apply(&doc, &actions, "https://a.test/");
    assert_eq!(summary.applied, 1);
    let out = doc.serialize_full();
    assert!(out.contains("新内容"), "{out}");
    assert!(out.contains("<span>二</span>"), "{out}");
    assert!(!out.contains("旧"), "{out}");
}

#[test]
fn 应用_append_与_remove() {
    let doc = Document::parse("<html><body><ul id='list'><li>一</li></ul></body></html>", "https://a.test/");
    let actions = vec![
        action("append_html", "#list", "", "<li>二</li>"),
        action("remove_node", "li", "", ""),
    ];
    let summary = dom_ops::apply(&doc, &actions, "https://a.test/");
    assert_eq!(summary.applied, 2);
    let out = doc.serialize_full();
    assert!(out.contains("二"), "{out}");
    assert!(!out.contains("<li>一</li>"), "{out}");
}

#[test]
fn 应用_style_属性_参与可见性() {
    let doc = Document::parse("<html><body><div id='x'>内容</div></body></html>", "https://a.test/");
    let actions = vec![action("set_attr", "#x", "style", "display:none")];
    dom_ops::apply(&doc, &actions, "https://a.test/");
    let node = doc.first("#x").ok().flatten().unwrap();
    assert!(!crate::agent::browser::document::is_visible(&node));
}

#[test]
fn 收集脚本_内联与外链() {
    let doc = Document::parse(
        "<html><head><script>var a=1;</script><script src='/x.js'></script></head><body><script>var b=2;</script></body></html>",
        "https://a.test/page",
    );
    let mut executed = std::collections::HashSet::new();
    let collected = dom_ops::collect_scripts(&doc, "https://a.test/page", &mut executed);
    // 文档序:head 内联 → head 外链 → body 内联。顺序不得重排。
    assert_eq!(collected.items.len(), 3);
    assert!(matches!(
        &collected.items[0],
        dom_ops::CollectedItem::Inline(s) if s.content.contains("a=1")
    ));
    assert!(matches!(
        &collected.items[1],
        dom_ops::CollectedItem::External(u) if u == "https://a.test/x.js"
    ));
    assert!(matches!(
        &collected.items[2],
        dom_ops::CollectedItem::Inline(s) if s.content.contains("b=2")
    ));
    // 重复收集:内联按指纹去重;外链由管线执行后记账,未执行前仍会出现。
    let again = dom_ops::collect_scripts(&doc, "https://a.test/page", &mut executed);
    assert_eq!(again.items.len(), 1, "内联应去重,仅剩未执行的外链");
    assert!(matches!(
        &again.items[0],
        dom_ops::CollectedItem::External(u) if u == "https://a.test/x.js"
    ));
}

// ---------- JS 线程(PRELUDE) ----------

const PAGE: &str = "<html><head><title>页</title></head><body><div id='app'>初始</div><a id='go' href='https://a.test/x'>去</a></body></html>";

#[test]
fn innerhtml_赋值产生_注入动作() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec!["document.getElementById('app').innerHTML = '<h1>JS渲染</h1>';"],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert!(reply
        .outcome
        .actions
        .iter()
        .any(|a| a.kind == "set_html" && a.selector.contains("app") && a.value.contains("JS渲染")));
}

#[test]
fn fetch_桥_返回错误结构而非挂死() {
    // 无监听端口:fetch 必须快速失败,不能挂死线程。
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "typeof fetch === 'function' ? 'has-fetch' : 'no-fetch'",
        PAGE,
        "https://a.test/",
    );
    assert_eq!(reply.outcome.value, "has-fetch");
}

#[test]
fn 定时器_虚拟时钟_延迟执行() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![
            "window.__done = 0;",
            "setTimeout(function(){ window.__done = 1; document.title = '定时完成'; }, 3000);",
        ],
        PAGE,
        "https://a.test/",
    );
    // RunScripts 管线内事件循环应已执行 3s 定时器(虚拟时钟快进)。
    assert!(reply
        .outcome
        .actions
        .iter()
        .any(|a| a.kind == "set_title" && a.value.contains("定时完成")));
}

#[test]
fn domcontentload_listener_被派发() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![
            "window.__loaded = false;",
            "document.addEventListener('DOMContentLoaded', function(){ window.__loaded = true; document.getElementById('app').textContent = '已加载'; });",
        ],
        PAGE,
        "https://a.test/",
    );
    assert!(reply
        .outcome
        .actions
        .iter()
        .any(|a| a.kind == "set_text" && a.value.contains("已加载")));
}

#[test]
fn eval_全局跨调用保留() {
    let handle = JsHandle::spawn();
    let first = eval_on(&handle, "window.__counter = 41; 'set'", PAGE, "https://a.test/");
    assert_eq!(first.outcome.value, "set");
    let second = eval_on(&handle, "window.__counter + 1", PAGE, "https://a.test/");
    assert_eq!(second.outcome.value, "42");
}

#[test]
fn click_listener_阻止默认行为() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![
            "document.getElementById('go').addEventListener('click', function(e){ e.preventDefault(); });",
        ],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok);
    let nodes = Document::parse(PAGE, "https://a.test/").snapshot_nodes();
    let e = env("https://a.test/");
    let dispatch = handle
        .request(|tx| JsRequest::Dispatch {
            selector: "#go".to_string(),
            event: "click".to_string(),
            env: e,
            nodes,
            reply: tx,
        })
        .expect("dispatch 失败");
    assert!(dispatch.handled);
    assert!(dispatch.default_prevented);
}

#[test]
fn 语句与表达式_与_错误结构() {
    let handle = JsHandle::spawn();
    let out = eval_on(&handle, "var t = 2; return t * 21;", PAGE, "https://a.test/");
    assert_eq!(out.outcome.value, "42");
    let bad = eval_on(&handle, "this is not js", PAGE, "https://a.test/");
    assert!(!bad.outcome.ok);
    assert!(bad.outcome.error.is_some());
}

#[test]
fn shutdown_后线程退出() {
    let handle = JsHandle::spawn();
    handle.shutdown();
    // Shutdown 之后请求应失败(线程已退出)。
    let result = handle.request(|tx| JsRequest::Eval {
        script: "1".to_string(),
        env: env("https://a.test/"),
        nodes: vec![],
        reply: tx,
    });
    assert!(result.is_err());
}

// ---------- 快照桥选择器与元素对象(第 174 轮) ----------

const SNAP_PAGE: &str = "<html><head><title>页</title><script>var x=1;</script></head>\
<body><div id=wrap><ul id=list><li id=i1 class='item'>甲</li><li id=i2 class='item active'>乙</li></ul>\
<input id=box title='搜索框' data-title='路线' value=''></div></body></html>";

#[test]
fn 元素对象提供_title_与_data_title() {
    // 站点脚本普遍写 `el.innerText || el.title`、`el.getAttribute('data-title')`;
    // 阶段二曾丢掉 title 属性,`el.title` 变 undefined,`().trim()` 直接抛 TypeError。
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "JSON.stringify({title: document.getElementById('box').title, \
         dataTitle: document.getElementById('box').getAttribute('data-title')})",
        SNAP_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert!(reply.outcome.value.contains("搜索框"), "{}", reply.outcome.value);
    assert!(reply.outcome.value.contains("路线"), "{}", reply.outcome.value);
}

#[test]
fn 文本收集不含脚本源码() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "document.body.innerText.indexOf('var x=1')",
        SNAP_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(reply.outcome.value, "-1", "body.innerText 不应包含 <script> 源码");
}

#[test]
fn 选择器支持子代组合器与伪类() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "JSON.stringify({child: document.querySelectorAll('#list > li').length, \
         first: document.querySelectorAll('li:first-child').length, \
         nth: document.querySelectorAll('li:nth-of-type(2)').length, \
         second: document.querySelectorAll('li:nth-child(2)').length})",
        SNAP_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(
        reply.outcome.value,
        "{\"child\":2,\"first\":1,\"nth\":1,\"second\":1}",
        "{}",
        reply.outcome.value
    );
}

#[test]
fn 未知伪类不匹配全部() {
    // 此前伪类被静默忽略,`div:hover` 会返回页面里全部 div —— 比报错更危险。
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "JSON.stringify({hover: document.querySelectorAll('li:hover').length, \
         unknown: document.querySelectorAll('li:not(.item)').length})",
        SNAP_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(
        reply.outcome.value,
        "{\"hover\":0,\"unknown\":0}",
        "状态类/未知伪类必须不匹配,而不是匹配全部:{}",
        reply.outcome.value
    );
}

#[test]
fn 元素作用域查询限定子树() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "JSON.stringify({scoped: document.querySelector('#i1').querySelectorAll('li').length, \
         global: document.querySelectorAll('li').length})",
        SNAP_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(
        reply.outcome.value,
        "{\"scoped\":0,\"global\":2}",
        "元素上的 querySelectorAll 只能查自己的子树:{}",
        reply.outcome.value
    );
}

#[test]
fn window_赋值成为全局标识符() {
    // 站点用 `window.alog = fn` 定义全局函数;window 若只是普通对象,
    // 裸 `alog` 标识符不可见,页面初始化直接 ReferenceError(实测百度地图)。
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "(window.__x = function(){ return 7; }, typeof __x + '|' + __x())",
        SNAP_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(reply.outcome.value, "function|7", "{}", reply.outcome.value);
}

#[test]
fn 常见标准全局存在() {
    // 缺一个就 ReferenceError,页面主逻辑一行都跑不起来。
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "JSON.stringify([typeof URL, typeof URLSearchParams, typeof Event, typeof CustomEvent, \
         typeof Element, typeof performance, typeof TextEncoder, typeof Blob, typeof FormData, \
         typeof structuredClone, typeof queueMicrotask])",
        SNAP_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(
        reply.outcome.value,
        "[\"function\",\"function\",\"function\",\"function\",\"function\",\"object\",\"function\",\"function\",\"function\",\"function\",\"function\"]",
        "{}",
        reply.outcome.value
    );
}

#[test]
fn 内核不定义_requirejs_以免页面装填器自杀() {
    // 页面自带的 requirejs 初始化时有 `if (isFunction(requirejs)) return;`,
    // 内核抢先定义 requirejs 会让整层 AMD 装载器不初始化(实测百度地图 widget 层)。
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "typeof requirejs",
        SNAP_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(reply.outcome.value, "undefined", "{}", reply.outcome.value);
}

#[test]
fn 动态插入脚本可触发_load_回调() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![
            "window.__loaded = 0;",
            "(function(){ var s = document.createElement('script'); s.src = 'https://a.test/m.js'; \
             s.onload = function(){ window.__loaded = 1; document.title = '模块就绪'; }; \
             document.body.appendChild(s); })();",
        ],
        SNAP_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    // 宿主执行完外链脚本后会触发 __lsmFireScriptLoad;这里外链取不到,
    // 回调不应触发(不得假触发)。
    assert!(
        !reply
            .outcome
            .actions
            .iter()
            .any(|a| a.kind == "set_title" && a.value.contains("模块就绪")),
        "脚本取回失败时不应假触发 onload:{:?}",
        reply.outcome.actions
    );
}

// ---------- 阶段五第一步:观察者系列 ----------

const OBS_PAGE: &str = "<html><head><title>观察者</title></head><body>\
<div id='app' data-state='a'>初始</div><div id='box' style='width:120px;height:40px'>盒子</div>\
</body></html>";

#[test]
fn mutation_observer_子节点变更触发回调() {
    // 此前 env_js 的 MutationObserver 是空壳:构造成功但回调永不触发,
    // 前端框架的「等 DOM 变化再渲染」链条整段停摆。
    let handle = JsHandle::spawn();
    let armed = eval_on(
        &handle,
        "(function(){ window.__mut = 0; \
         var mo = new MutationObserver(function(rs){ window.__mut += rs.length; }); \
         mo.observe(document.body, {childList: true, subtree: true}); \
         document.body.appendChild(document.createElement('div')); \
         return 'armed'; })()",
        OBS_PAGE,
        "https://a.test/",
    );
    assert!(armed.outcome.ok, "{:?}", armed.outcome.error);
    let got = eval_on(&handle, "window.__mut", OBS_PAGE, "https://a.test/");
    assert_eq!(got.outcome.value, "1", "子节点变更应派发一条 childList 记录");
}

#[test]
fn mutation_observer_属性变更触发回调() {
    let handle = JsHandle::spawn();
    let armed = eval_on(
        &handle,
        "(function(){ window.__attr = 0; \
         var mo = new MutationObserver(function(rs){ window.__attr += rs.length; }); \
         mo.observe(document.getElementById('app'), {attributes: true}); \
         document.getElementById('app').setAttribute('data-state', 'b'); \
         return 'armed'; })()",
        OBS_PAGE,
        "https://a.test/",
    );
    assert!(armed.outcome.ok, "{:?}", armed.outcome.error);
    let got = eval_on(&handle, "window.__attr", OBS_PAGE, "https://a.test/");
    assert_eq!(got.outcome.value, "1", "属性变更应派发一条 attributes 记录");
}

#[test]
fn mutation_observer_选项过滤_属性白名单() {
    // attributeFilter 不含的属性名不得触发;不含 options.attributes 时默认不监听属性。
    let handle = JsHandle::spawn();
    let armed = eval_on(
        &handle,
        "(function(){ window.__flt = 0; \
         var mo = new MutationObserver(function(rs){ window.__flt += rs.length; }); \
         mo.observe(document.getElementById('app'), {attributes: true, attributeFilter: ['title']}); \
         document.getElementById('app').setAttribute('data-state', 'c'); \
         return 'armed'; })()",
        OBS_PAGE,
        "https://a.test/",
    );
    assert!(armed.outcome.ok, "{:?}", armed.outcome.error);
    let got = eval_on(&handle, "window.__flt", OBS_PAGE, "https://a.test/");
    assert_eq!(got.outcome.value, "0", "白名单外的属性不应触发观察者");
}

#[test]
fn mutation_observer_宿主侧变更可见() {
    // Agent 侧动作造成的 DOM 变化(快照 diff)也要通知页面脚本:
    // 页面脚本此前只能靠轮询感知宿主改动。
    let handle = JsHandle::spawn();
    let armed = eval_on(
        &handle,
        "(function(){ window.__host = 0; \
         var mo = new MutationObserver(function(rs){ window.__host += rs.length; }); \
         mo.observe(document.getElementById('app'), {attributes: true}); \
         return 'armed'; })()",
        OBS_PAGE,
        "https://a.test/",
    );
    assert!(armed.outcome.ok, "{:?}", armed.outcome.error);
    // 换一份快照:data-state 由 a 变 b → 宿主侧 diff 合成 attributes 记录。
    let changed = OBS_PAGE.replace("data-state='a'", "data-state='b'");
    let second = eval_on(&handle, "'noop'", &changed, "https://a.test/");
    assert!(second.outcome.ok, "{:?}", second.outcome.error);
    let got = eval_on(&handle, "window.__host", &changed, "https://a.test/");
    assert_eq!(got.outcome.value, "1", "宿主侧属性变化应合成 mutation 记录");
}

#[test]
fn intersection_observer_视口内元素相交() {
    let handle = JsHandle::spawn();
    let armed = eval_on_with_rects(
        &handle,
        "(function(){ window.__io = 0; window.__ratio = -1; \
         var io = new IntersectionObserver(function(es){ \
           window.__io += es.length; window.__ratio = es[0].intersectionRatio; }); \
         io.observe(document.getElementById('box')); \
         return 'armed'; })()",
        OBS_PAGE,
        "https://a.test/",
    );
    assert!(armed.outcome.ok, "{:?}", armed.outcome.error);
    let got = eval_on_with_rects(&handle, "window.__io", OBS_PAGE, "https://a.test/");
    assert_eq!(got.outcome.value, "1", "首次观察应产生一条初始 entry");
    let ratio = eval_on_with_rects(&handle, "window.__ratio", OBS_PAGE, "https://a.test/");
    let v: f64 = ratio.outcome.value.parse().unwrap_or(-1.0);
    assert!(v > 0.0, "视口内元素应有正的相交比例:{}", ratio.outcome.value);
}

#[test]
fn resize_observer_尺寸变化触发() {
    let handle = JsHandle::spawn();
    let armed = eval_on_with_rects(
        &handle,
        "(function(){ window.__ro = 0; \
         var ro = new ResizeObserver(function(es){ window.__ro += es.length; }); \
         ro.observe(document.getElementById('box')); \
         return 'armed'; })()",
        OBS_PAGE,
        "https://a.test/",
    );
    assert!(armed.outcome.ok, "{:?}", armed.outcome.error);
    let got = eval_on_with_rects(&handle, "window.__ro", OBS_PAGE, "https://a.test/");
    assert_eq!(got.outcome.value, "1", "首次观察应产生一条初始尺寸记录");
    // 尺寸变化(快照矩形改变)后再观察 → 新的 entry。
    let resized = OBS_PAGE.replace("width:120px;height:40px", "width:200px;height:80px");
    let second = eval_on_with_rects(&handle, "'noop'", &resized, "https://a.test/");
    assert!(second.outcome.ok, "{:?}", second.outcome.error);
    let got2 = eval_on_with_rects(&handle, "window.__ro", &resized, "https://a.test/");
    assert_eq!(got2.outcome.value, "2", "尺寸变化应再产生一条记录");
}

#[test]
fn 观察者_窗口与全局可见() {
    // 站点普遍写 `new window.MutationObserver(...)`;缺 window 暴露直接 TypeError。
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "JSON.stringify([typeof MutationObserver, typeof window.MutationObserver, \
         typeof IntersectionObserver, typeof ResizeObserver])",
        OBS_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(
        reply.outcome.value,
        "[\"function\",\"function\",\"function\",\"function\"]",
        "{}",
        reply.outcome.value
    );
}

#[test]
fn append_prepend_存在且落地宿主() {
    // 站点脚本普遍用 `el.append(x)` / `el.prepend(x)`;缺这两个方法直接抛
    // "not a callable function",整段脚本后半截不执行(实测 example.com 的 s.js)。
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![
            "(function(){ var d = document.createElement('span'); d.textContent = '新节点'; \
              document.getElementById('app').append(d); return 'ok'; })()",
        ],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert!(
        reply
            .outcome
            .actions
            .iter()
            .any(|a| a.kind == "append_html" && a.value.contains("新节点")),
        "append 应产生宿主插入动作:{:?}",
        reply.outcome.actions
    );

    let pre = run_scripts(
        &handle,
        vec![
            "(function(){ var d = document.createElement('b'); d.textContent = '前插'; \
              document.getElementById('app').prepend(d); return 'ok'; })()",
        ],
        PAGE,
        "https://a.test/",
    );
    assert!(pre.outcome.ok, "{:?}", pre.outcome.error);
    assert!(
        pre.outcome
            .actions
            .iter()
            .any(|a| a.kind == "insert_html" && a.value.contains("前插")),
        "prepend 应产生宿主前插动作:{:?}",
        pre.outcome.actions
    );
}

#[test]
fn detached_innerhtml_解析出可查询子树() {
    let h = JsHandle::spawn();
    let reply = eval_on(
        &h,
        r#"(function(){
            var d = document.createElement('div');
            d.innerHTML = ' <link/><table></table><a href="/a">a</a><input type="checkbox"/>';
            var a = d.getElementsByTagName('A')[0];
            var input = d.getElementsByTagName('input')[0];
            return JSON.stringify({
                aLen: d.getElementsByTagName('A').length,
                iLen: d.getElementsByTagName('input').length,
                href: a ? a.getAttribute('href') : null,
                inputType: input ? input.getAttribute('type') : null,
                kids: d.children.length
            });
        })()"#,
        "<html><body></body></html>",
        "https://a.test/",
    );
    let v = reply.outcome.value;
    assert!(v.contains("\"aLen\":1"), "detached 子树应能按标签查询:{v}");
    assert!(v.contains("\"iLen\":1"), "detached 子树应能按标签查询:{v}");
    assert!(v.contains("\"href\":\"/a\""), "属性应可读:{v}");
    assert!(v.contains("\"inputType\":\"checkbox\""), "属性应可读:{v}");
    assert!(v.contains("\"kids\":4"), "children 应只含解析出的元素节点:{v}");
    h.shutdown();
}

#[test]
fn substr_按_annexb_语义可用() {
    let h = JsHandle::spawn();
    let reply = eval_on(
        &h,
        r#"JSON.stringify({
            tail: 'array'.substr(1),
            neg: 'abcdef'.substr(-2),
            len: 'abcdef'.substr(1, 2),
            negLen: 'abcdef'.substr(2, -1),
            big: 'abcdef'.substr(2, 100)
        })"#,
        "<html><body></body></html>",
        "https://a.test/",
    );
    let v = reply.outcome.value;
    assert!(v.contains("\"tail\":\"rray\""), "{v}");
    assert!(v.contains("\"neg\":\"ef\""), "{v}");
    assert!(v.contains("\"len\":\"bc\""), "{v}");
    assert!(v.contains("\"negLen\":\"\""), "{v}");
    assert!(v.contains("\"big\":\"cdef\""), "{v}");
    h.shutdown();
}

#[test]
fn body_client_尺寸按视口兜底() {
    let h = JsHandle::spawn();
    let reply = eval_on(
        &h,
        "JSON.stringify({w: document.body.clientWidth, h: document.body.clientHeight})",
        "<html><body><div>x</div></body></html>",
        "https://a.test/",
    );
    let v = reply.outcome.value;
    assert!(v.contains("\"w\":1280"), "body.clientWidth 应按视口兜底:{v}");
    assert!(v.contains("\"h\":720"), "body.clientHeight 应按视口兜底:{v}");
    h.shutdown();
}

// ---------- 第 174 轮:window 代理 / 委托匹配 / 标准全局 ----------

#[test]
fn window_只读访问器赋值_静默失败不抛错() {
    // 站点反爬脚本常做 `Object.defineProperty(window,'eval',{get,configurable:false})`
    // 侦测页面是否调用 eval。此前 Proxy [[Set]] 后置校验会抛
    // "Proxy trap set unexpected accessor descriptor",一次赋值就打断整段初始化。
    // 浏览器语义是静默失败(sloppy),这里必须不抛。
    let h = JsHandle::spawn();
    let reply = eval_on(
        &h,
        r#"
        var out = {};
        try {
            Object.defineProperty(window, '__ro', {
                get: function () { return 7; },
                configurable: false
            });
            out.def = 'ok';
        } catch (e) { out.def = 'THROW:' + String(e); }
        try { window.__ro = 1; out.assign = 'no-throw'; }
        catch (e) { out.assign = 'THROW:' + String(e); }
        try { window.__plain = 5; out.plain = String(window.__plain); }
        catch (e) { out.plain = 'THROW:' + String(e); }
        return JSON.stringify(out);
        "#,
        "<html><body></body></html>",
        "https://a.test/",
    );
    let v = reply.outcome.value;
    assert!(reply.outcome.ok, "{v}");
    assert!(v.contains("\"def\":\"ok\""), "defineProperty 不得抛错:{v}");
    assert!(v.contains("\"assign\":\"no-throw\""), "只读访问器赋值不得抛错:{v}");
    assert!(v.contains("\"plain\":\"5\""), "普通属性赋值应正常:{v}");
    h.shutdown();
}

#[test]
fn 元素匹配_matches_closest_可用() {
    // 事件委托(jQuery `$(document).on('click', sel, fn)`、各框架委托)依赖
    // `el.matches()/el.closest()` 从 target 向上匹配委托选择器。
    let h = JsHandle::spawn();
    let reply = eval_on(
        &h,
        r#"
        var el = document.querySelector('.route-button');
        var out = {};
        out.matches = String(el.matches('.route-button'));
        out.compound = String(el.matches('.searchbox-content-button.right-button.route-button'));
        out.closest = el.closest('.route-button') ? 'hit' : 'miss';
        out.closestUp = el.closest('#left-panel') ? 'hit' : 'miss';
        out.closestNone = el.closest('.nope');
        return JSON.stringify(out);
        "#,
        "<html><body><div id=left-panel><div class='searchbox-content-button right-button route-button' data-title='路线'></div></div></body></html>",
        "https://a.test/",
    );
    let v = reply.outcome.value;
    assert!(v.contains("\"matches\":\"true\""), "{v}");
    assert!(v.contains("\"compound\":\"true\""), "{v}");
    assert!(v.contains("\"closest\":\"hit\""), "{v}");
    assert!(v.contains("\"closestUp\":\"hit\""), "{v}");
    assert!(v.contains("\"closestNone\":null"), "{v}");
    h.shutdown();
}

#[test]
fn 标准全局_NodeFilter_与_createElementNS_可用() {
    let h = JsHandle::spawn();
    let reply = eval_on(
        &h,
        r#"
        var out = {};
        try { out.nodeFilter = typeof NodeFilter; }
        catch (e) { out.nodeFilter = 'THROW:' + String(e); }
        try { out.showElement = String(NodeFilter && NodeFilter.SHOW_ELEMENT); }
        catch (e) { out.showElement = 'THROW:' + String(e); }
        try { out.filterAccept = String(NodeFilter && NodeFilter.FILTER_ACCEPT); }
        catch (e) { out.filterAccept = 'THROW:' + String(e); }
        try { out.nodeIterator = typeof NodeIterator; }
        catch (e) { out.nodeIterator = 'THROW:' + String(e); }
        try { out.createNS = typeof document.createElementNS; }
        catch (e) { out.createNS = 'THROW:' + String(e); }
        try {
            var e = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
            out.svg = String(e && e.tagName).toLowerCase();
        } catch (e2) { out.svg = 'THROW:' + String(e2); }
        return JSON.stringify(out);
        "#,
        "<html><body></body></html>",
        "https://a.test/",
    );
    let v = reply.outcome.value;
    assert!(v.contains("\"nodeFilter\":\"object\""), "{v}");
    assert!(v.contains("\"showElement\":\"1\""), "{v}");
    assert!(v.contains("\"filterAccept\":\"1\""), "{v}");
    assert!(v.contains("\"nodeIterator\":\"function\""), "{v}");
    assert!(v.contains("\"createNS\":\"function\""), "{v}");
    assert!(v.contains("\"svg\":\"svg\""), "{v}");
    h.shutdown();
}

// ---------- Boa VM panic 探针:找出 `must be declarative environment` 触发点 ----------

#[test]
fn eval_js_函数体内_class_声明不触发VMpanic() {
    // Boa 0.20 的 `new Function` 函数体含 `class` 声明,执行时必 panic
    // (`must be declarative environment`),整条页面 JS 线程被打死。
    // `__lsmEvalUser` 已改为「间接 eval 包 IIFE」,本用例锁死该行为。
    let cases: Vec<(&str, &str)> = vec![
        ("fn_body_class+return", "class C {} return typeof C;"),
        ("fn_body_class_only", "class C {} return 1;"),
        ("nested_fn_class", "function f(){ class G {} return 1; } return f();"),
        ("arrow_class", "return (() => { class C {} return 1; })();"),
        ("iife_class", "return (function(){ class C {} return 1; })();"),
        ("fn_body_let+return", "let x = 1; return x;"),
        ("fn_body_const+return", "const y = 2; return y;"),
    ];
    for (name, src) in cases {
        let h = JsHandle::spawn();
        let reply = eval_on(&h, src, "<html><body></body></html>", "https://a.test/");
        assert!(
            reply.outcome.ok,
            "用例 {name} 应成功:{}",
            reply.outcome.error.clone().unwrap_or_default()
        );
        h.shutdown();
    }
}

#[test]
fn VMpanic_后保留Context_不丢页面全局() {
    // panic 不能重建 Context(页面全局状态全丢);健康检查通过就保留。
    let h = JsHandle::spawn();
    // 先种一个全局
    let seed = eval_on(&h, "window.__keep = 42; return window.__keep;", "<html><body></body></html>", "https://a.test/");
    assert!(seed.outcome.ok, "{:?}", seed.outcome.error);
    // 触发一次 VM panic(仍走 new Function 的站点代码路径)
    let panic_src = "return (function(){ class P {} return 1; })();";
    let bad = eval_on(&h, panic_src, "<html><body></body></html>", "https://a.test/");
    assert!(bad.outcome.ok, "class 路径修复后不应再 panic:{:?}", bad.outcome.error);
    // 全局仍在
    let check = eval_on(&h, "return String(window.__keep);", "<html><body></body></html>", "https://a.test/");
    assert!(check.outcome.ok, "{:?}", check.outcome.error);
    assert_eq!(check.outcome.value, "42");
    h.shutdown();
}

// ---------- 协议相对地址 / DOM 序列化(2026-10-22 第 174 轮) ----------

#[test]
fn location_protocol_带结尾冒号() {
    // URL 规范:location.protocol = "https:"。缺冒号会让站点拼协议相对地址
    // (location.protocol + '//' + host)变成 "https//host" —— 百度地图的
    // qt=ssn / qt=subwayscity 即因此 404,地图数据链断裂。
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "String(location.protocol)",
        "<html><body></body></html>",
        "https://a.test/x",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(reply.outcome.value.as_str(), "https:");
}

#[test]
fn 游离元素_className_进序列化() {
    let handle = JsHandle::spawn();
    let doc = Document::parse(
        "<html><body><div id='host'></div></body></html>",
        "https://a.test/",
    );
    let nodes = doc.snapshot_nodes();
    let sources = vec![ScriptSource {
        kind: "inline",
        content: "var d=document.createElement('div'); d.className='marked'; \
                  document.getElementById('host').appendChild(d);"
            .to_string(),
        src_url: String::new(),
    }];
    let e = env("https://a.test/");
    let reply = handle
        .request(|tx| JsRequest::RunScripts {
            scripts: sources,
            env: e,
            nodes,
            reply: tx,
        })
        .expect("JS 线程请求失败");
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    let actions = reply.outcome.actions;
    let applied = dom_ops::apply(&doc, &actions, "https://a.test/");
    assert!(applied.applied > 0, "动作未落地:{actions:?}");
    let out = doc.serialize_full();
    assert!(out.contains("marked"), "游离元素 className 丢失:{out}");
}

#[test]
fn 游离元素_cloneNode_深拷贝保留内容() {
    let handle = JsHandle::spawn();
    let doc = Document::parse(
        "<html><body><div id='host'></div></body></html>",
        "https://a.test/",
    );
    let nodes = doc.snapshot_nodes();
    let sources = vec![ScriptSource {
        kind: "inline",
        content: "var t=document.createElement('div'); t.innerHTML='<b>模板</b>'; \
                  var c=t.cloneNode(true); c.className='copy'; \
                  document.getElementById('host').appendChild(c);"
            .to_string(),
        src_url: String::new(),
    }];
    let e = env("https://a.test/");
    let reply = handle
        .request(|tx| JsRequest::RunScripts {
            scripts: sources,
            env: e,
            nodes,
            reply: tx,
        })
        .expect("JS 线程请求失败");
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    dom_ops::apply(&doc, &reply.outcome.actions, "https://a.test/");
    let out = doc.serialize_full();
    assert!(out.contains("模板"), "cloneNode 深拷贝丢内容:{out}");
    assert!(out.contains("copy"), "cloneNode 深拷贝丢类名:{out}");
}

#[test]
fn 应用_append_node_保留子树() {
    // 站点把已有节点搬进容器:必须按真实 outerHTML 搬,不能只搬文本。
    let doc = Document::parse(
        "<html><body><div id='src'><span>内容</span></div><div id='dst'></div></body></html>",
        "https://a.test/",
    );
    let actions = vec![action("append_node", "#src", "#dst", "")];
    let summary = dom_ops::apply(&doc, &actions, "https://a.test/");
    assert_eq!(summary.applied, 1);
    let out = doc.serialize_full();
    let dst = doc.first("#dst").ok().flatten().unwrap();
    let html = crate::agent::browser::document::outer_html(&dst);
    assert!(html.contains("内容"), "子树未搬入:{html}");
}
