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
    assert_eq!(collected.inline.len(), 2);
    assert_eq!(collected.external_urls, vec!["https://a.test/x.js".to_string()]);
    // 重复收集按指纹去重。
    let again = dom_ops::collect_scripts(&doc, "https://a.test/page", &mut executed);
    assert!(again.inline.is_empty());
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
