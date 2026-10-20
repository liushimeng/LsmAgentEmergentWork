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
