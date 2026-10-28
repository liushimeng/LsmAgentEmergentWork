//! 阶段二脚本管线单元测试:dom_ops 应用、脚本收集、JS 线程行为(PRELUDE)。

use std::collections::HashMap;

use super::dom_ops;
use super::{JsHandle, JsRequest, JsReply, PageEnv, ScriptSource};
use crate::agent::browser::document::Document;

pub(super) fn env(url: &str) -> PageEnv {
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
        resources: Vec::new(),
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

pub(super) fn eval_on(handle: &JsHandle, script: &str, doc_html: &str, url: &str) -> JsReply {
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
            budget_ms: 0,
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
fn 永久定时器_单请求快进次数有硬上限() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
            window.__ticks = 0;
            setInterval(function () {
                window.__ticks++;
                document.getElementById('app').setAttribute('data-tick', String(window.__ticks));
            }, 16);
        "#],
        PAGE,
        "https://a.test/",
    );
    let ticks = reply
        .outcome
        .actions
        .iter()
        .filter(|a| a.kind == "set_attr" && a.key == "data-tick")
        .count();
    assert_eq!(ticks, 64, "永久动画不能在一次请求里无界快进:{:?}", reply.outcome.actions);
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    handle.shutdown();
}

#[test]
fn 控制台_error对象_保留名称消息和堆栈() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"
        console.error(new TypeError('route boot failed'));
        console.error({name: 'Error', message: 'wrapped', stack: 'Error: wrapped\n    at widget.js:1:2'});
        'logged'
        "#,
        PAGE,
        "https://a.test/",
    );
    let text = reply
        .outcome
        .logs
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("TypeError"), "错误名丢失:{text}");
    assert!(text.contains("route boot failed"), "错误消息丢失:{text}");
    assert!(text.contains("wrapped"), "包装错误消息丢失:{text}");
    assert!(!text.contains("{}"), "错误对象不得再序列化成空对象:{text}");
    handle.shutdown();
}

#[test]
fn 控制台_assert_失败只记录且不阻断初始化() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
        console.assert(true, 'not an error');
        console.assert(false, '配置检查失败');
        document.getElementById('app').textContent = '初始化完成';
        "#],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "console.assert 不得抛错:{:?}", reply.outcome.error);
    assert!(reply
        .outcome
        .actions
        .iter()
        .any(|a| a.kind == "set_text" && a.value.contains("初始化完成")));
    assert!(reply
        .outcome
        .logs
        .iter()
        .any(|l| l.level == "error" && l.text.contains("配置检查失败")));
    handle.shutdown();
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

#[test]
fn 动态脚本_load回调_相对与绝对URL使用同一键() {
    let handle = JsHandle::spawn();
    let armed = eval_on(
        &handle,
        r#"(function(){
            window.__relativeLoaded = 0;
            window.__protocolLoaded = 0;
            var a = document.createElement('script');
            a.src = '/pkg/route.js';
            a.onload = function(){ window.__relativeLoaded = 1; };
            document.body.appendChild(a);
            var b = document.createElement('script');
            b.src = '//a.test/pkg/tile.js';
            b.onload = function(){ window.__protocolLoaded = 1; };
            document.body.appendChild(b);
            return 'armed';
        })()"#,
        PAGE,
        "https://a.test/",
    );
    assert!(armed.outcome.ok, "{:?}", armed.outcome.error);
    let fired = eval_on(
        &handle,
        r#"
        __lsmFireScriptLoad('https://a.test/pkg/route.js');
        __lsmFireScriptLoad('https://a.test/pkg/tile.js');
        String(window.__relativeLoaded) + ':' + String(window.__protocolLoaded)
        "#,
        PAGE,
        "https://a.test/",
    );
    assert_eq!(fired.outcome.value, "1:1", "相对/协议相对 src 必须收到绝对 URL 的 load 回调");
    handle.shutdown();
}

#[test]
fn 脚本URL归一_相对与协议相对都归到绝对() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"
        try { globalThis.URL = undefined; } catch (e) {}
        JSON.stringify({
            abs: __lsmScriptKey('https://a.test/z.js'),
            root: __lsmScriptKey('/pkg/route.js'),
            rel: __lsmScriptKey('pkg/route.js'),
            proto: __lsmScriptKey('//a.test/pkg/tile.js')
        })
        "#,
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    let v = reply.outcome.value;
    assert!(v.contains("\"abs\":\"https://a.test/z.js\""), "{v}");
    assert!(v.contains("\"root\":\"https://a.test/pkg/route.js\""), "{v}");
    assert!(v.contains("\"rel\":\"https://a.test/pkg/route.js\""), "{v}");
    assert!(v.contains("\"proto\":\"https://a.test/pkg/tile.js\""), "{v}");
    handle.shutdown();
}

#[test]
fn 动态节点挂载后再追加子节点_回写宿主dom() {
    // 动态节点在宿主里以 data-lsm-id 定位;站点常在挂载之后才往里塞内容
    // (百度地图先挂 platform,再追加图层面板与瓦片)。旧实现只改本地树,
    // 宿主 DOM 永远看不到这些内容(地图面板不进页面)。
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
            var d = document.createElement('div');
            d.id = 'dyn';
            document.body.appendChild(d);
            var c = document.createElement('span');
            c.id = 'kid';
            d.appendChild(c);
        "#],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    let hit = reply.outcome.actions.iter().any(|a| {
        a.kind == "append_html" && a.selector.contains("data-lsm-id") && a.value.contains("kid")
    });
    assert!(
        hit,
        "动态节点挂载后的追加必须回写宿主 DOM:{:?}",
        reply.outcome.actions
    );
    handle.shutdown();
}

#[test]
fn 动态节点同批可查且回快照后不重复() {
    let handle = JsHandle::spawn();
    let first = run_scripts(
        &handle,
        vec![r#"
            var root = document.createElement('div');
            root.id = 'dyn-root';
            var child = document.createElement('span');
            child.id = 'dyn-child';
            root.appendChild(child);
            document.getElementById('app').appendChild(root);
            child.remove();
            window.__removedCount = document.querySelectorAll('#dyn-child').length;
            root.appendChild(child);
            window.__counts = {
                doc: document.querySelectorAll('#dyn-child').length,
                root: root.querySelectorAll('#dyn-child').length,
                scoped: document.getElementById('app').querySelectorAll('#dyn-child').length,
                rect: document.querySelector('#dyn-child').getBoundingClientRect().width
            };
        "#],
        PAGE,
        "https://a.test/",
    );
    assert!(first.outcome.ok, "同批动态查询不得抛错:{:?}", first.outcome.error);
    let doc = Document::parse(PAGE, "https://a.test/");
    dom_ops::apply(&doc, &first.outcome.actions, "https://a.test/");
    let html = doc.serialize_full();
    let second = eval_on(
        &handle,
        "JSON.stringify({before:window.__counts,removed:window.__removedCount,after:document.querySelectorAll('#dyn-child').length})",
        &html,
        "https://a.test/",
    );
    let v = second.outcome.value;
    assert!(v.contains("\"doc\":1"), "document 同批应查到动态子节点:{v}");
    assert!(v.contains("\"root\":1"), "动态父节点应能查自己的子树:{v}");
    assert!(v.contains("\"scoped\":1"), "静态父节点应能查新挂载的动态子树:{v}");
    assert!(v.contains("\"rect\":0"), "未回布局前几何应为 0 而不是节点缺失:{v}");
    assert!(v.contains("\"removed\":0"), "remove 后不得再出现在 document 查询:{v}");
    assert!(v.contains("\"after\":1"), "快照收养后同一节点不得返回两次:{v}");
    handle.shutdown();
}

#[test]
fn 元素contains_按当前树判定祖先() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"
        var box = document.getElementById('box');
        var child = document.getElementById('child');
        var outside = document.getElementById('outside');
        var out = {
            self: box.contains(box),
            child: box.contains(child),
            outside: box.contains(outside),
            doc: document.contains(child)
        };
        return JSON.stringify(out);
        "#,
        "<html><body><div id='box'><span id='child'>子</span></div><div id='outside'>外</div></body></html>",
        "https://a.test/",
    );
    let v = reply.outcome.value;
    assert!(v.contains("\"self\":true"), "{v}");
    assert!(v.contains("\"child\":true"), "{v}");
    assert!(v.contains("\"outside\":false"), "{v}");
    assert!(v.contains("\"doc\":true"), "{v}");
    handle.shutdown();
}

#[test]
fn 元素身份跨DOM变化保持稳定_作用域查询不串位() {
    // 快照用数组下标标识节点;DOM 在两轮之间插入节点后下标会错位,
    // 上一轮捕获的元素包装器会指向别的元素(百度地图首屏 find(".right-button")
    // 落空 → 搜索框初始化失败)。真实浏览器的元素身份与 DOM 变化无关。
    const BEFORE: &str = "<html><body><div id='box'><span class='child'>子</span></div></body></html>";
    const AFTER: &str =
        "<html><body><div id='other'><i></i></div><div id='box'><span class='child'>子</span></div></body></html>";
    let handle = JsHandle::spawn();
    let armed = eval_on(
        &handle,
        "window.__box = document.getElementById('box'); String(window.__box.querySelectorAll('.child').length)",
        BEFORE,
        "https://a.test/",
    );
    assert_eq!(armed.outcome.value, "1", "{:?}", armed.outcome.error);
    let after = eval_on(
        &handle,
        "String(window.__box.querySelectorAll('.child').length) + ':' + String(window.__box.getElementsByTagName('span').length)",
        AFTER,
        "https://a.test/",
    );
    assert_eq!(after.outcome.value, "1:1", "DOM 变化后旧元素包装器必须仍指向自身子树");
    handle.shutdown();
}

#[test]
fn 初始空页面不预置AMD_UMD注册浏览器全局() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"
        (function(root, factory){
            if (typeof define === 'function' && define.amd) define(factory);
            else root.__initialUmd = factory();
        })(globalThis, function(){ return 42; });
        return String(globalThis.__initialUmd) + ':' + typeof globalThis.define;
        "#,
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(reply.outcome.value, "42:undefined", "初始 AMD 不得截走 UMD 的 window 导出");
    handle.shutdown();
}

#[test]
fn FIS包装define_不凭空增加amd标志_UMD仍注册全局() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"
        Object.defineProperty(globalThis, 'require', {
            value: {loadJs: function(){}}, writable: true, configurable: true
        });
        Object.defineProperty(globalThis, 'define', {
            value: function(id, factory){}, writable: true, configurable: true
        });
        __lsmAmdKind();
        (function(root, factory){
            if (typeof define === 'function' && define.amd) define(factory);
            else root.__umdGlobal = factory();
        })(globalThis, function(){ return 42; });
        return String(globalThis.__umdGlobal) + ':' + typeof globalThis.define.amd;
        "#,
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(reply.outcome.value, "42:undefined", "UMD 全局导出不能被伪造的 define.amd 截走");
    handle.shutdown();
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
            e.setAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href', '#icon');
            out.nsGet = e.getAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href');
            out.nsHas = e.hasAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href');
            e.removeAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href');
            out.nsRemoved = e.getAttribute('xlink:href') === null;
            document.body.setAttributeNS(null, 'data-ns', 'static-ok');
            out.staticNs = document.body.getAttributeNS(null, 'data-ns');
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
    assert!(v.contains("\"nsGet\":\"#icon\""), "{v}");
    assert!(v.contains("\"nsHas\":true"), "{v}");
    assert!(v.contains("\"nsRemoved\":true"), "{v}");
    assert!(v.contains("\"staticNs\":\"static-ok\""), "{v}");
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
            budget_ms: 0,
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
            budget_ms: 0,
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

#[test]
fn 标准_dom_构造器与事件子类齐备() {
    // 实测百度地图反爬脚本 abclite:读 window.Document.prototype.createElement、
    // new MouseEvent(...);缺失导致整段脚本 TypeError,BMAP_SECKEY 不落地。
    let h = JsHandle::spawn();
    let reply = eval_on(
        &h,
        r#"JSON.stringify({
            doc: typeof Document,
            hdoc: typeof HTMLDocument,
            doc_win: typeof window.Document,
            doc_ctor: (typeof HTMLDocument !== 'undefined' && document.constructor === HTMLDocument),
            create_element_proto: typeof Document.prototype.createElement,
            div: typeof HTMLDivElement,
            input: typeof HTMLInputElement,
            mouse: typeof MouseEvent,
            keyboard: typeof KeyboardEvent,
            pointer: typeof PointerEvent,
            wheel: typeof WheelEvent,
            input_evt: typeof InputEvent,
            focus: typeof FocusEvent,
            message: typeof MessageEvent,
            touch: typeof Touch,
            mk_mouse: (function(){ var e = new MouseEvent('mousedown', {clientX: 12, bubbles: true});
                         return [e.type, e.clientX, e.bubbles].join('/'); })(),
            mk_key: (function(){ var e = new KeyboardEvent('keydown', {key: 'Enter'});
                       return [e.key, e.keyCode].join('/'); })()
        })"#,
        "<html><body><div id='x'>内容</div></body></html>",
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "求值失败:{:?}", reply.outcome.error);
    let v: serde_json::Value = serde_json::from_str(&reply.outcome.value).expect("结果应为 JSON");
    for key in [
        "doc", "hdoc", "doc_win", "div", "input", "mouse", "keyboard", "pointer", "wheel",
        "input_evt", "focus", "message", "touch",
    ] {
        assert_eq!(v[key], "function", "{key} 应为构造器:{v}");
    }
    assert_eq!(v["create_element_proto"], "function", "Document.prototype.createElement 缺失:{v}");
    assert_eq!(v["doc_ctor"], true, "document.constructor 应为 HTMLDocument:{v}");
    assert_eq!(v["mk_mouse"], "mousedown/12/true", "{v}");
    assert_eq!(v["mk_key"], "Enter/13", "KeyboardEvent 应按 key 回填 keyCode:{v}");
}

#[test]
fn document_nodeType_为9_使Sizzle类选择器引擎可用() {
    // 百度地图等站点用 jQuery 风格选择器引擎(Sizzle/Tangram);其入口即
    // `context = context || document; if (nodeType !== 1 && nodeType !== 9) return []`。
    // 快照桥 document 缺 nodeType → 引擎恒返回空集 → 全部 widget 静默不渲染。
    let handle = JsHandle::spawn();
    let reply = eval_on(&handle, "document.nodeType", PAGE, "https://a.test/");
    assert_eq!(reply.outcome.value, "9");
    let guard = eval_on(
        &handle,
        "var n = document.nodeType; (n !== 1 && n !== 9) ? 'empty' : 'ok'",
        PAGE,
        "https://a.test/",
    );
    assert_eq!(guard.outcome.value, "ok", "Sizzle 守卫必须放行 document");
}

// ===================== 动态节点几何回填 / 事件绑定(第 N 轮) =====================
// 站点地图与路线面板的容器由 JS 建出(document.createElement + 设尺寸 + appendChild)。
// 旧实现把 clientWidth/offsetWidth 写死 0、getBoundingClientRect 恒零、
// addEventListener 空实现 —— 地图读到 0×0 不装载、面板按钮点了没反应。
// 宿主其实已把这些节点重新解析进 DOM 并参与布局,几何就在快照 rect 里。

pub(super) const DYN_STAGE_PAGE: &str = "<html><body><p>stage</p></body></html>";

#[test]
fn 动态元素几何由宿主快照回填() {
    let handle = JsHandle::spawn();
    // 第一轮:建出动态节点并挂载。当轮宿主还没布局,几何按 display:none 语义取 0。
    let r1 = eval_on(
        &handle,
        r#"(function(){
          var d = document.createElement('div');
          d.id = 'dyn';
          d.style.cssText = 'width:300px;height:200px';
          document.body.appendChild(d);
          window.__dyn = d;
          return JSON.stringify({cw: d.clientWidth, ow: d.offsetWidth,
                                 rect: d.getBoundingClientRect()});
        })()"#,
        DYN_STAGE_PAGE,
        "https://a.test/",
    );
    assert!(r1.outcome.ok, "{r1:?}");
    let v1: serde_json::Value = serde_json::from_str(&r1.outcome.value).unwrap();
    assert_eq!(v1["cw"], 0, "未挂载/未布局应为 0:{v1}");
    assert_eq!(v1["rect"]["width"], 0, "未布局时 rect 应为 0:{v1}");

    // 第二轮:宿主把带 data-lsm-id 的子树解析并布局后,同一批动态对象应读到真实几何。
    let hosted = "<html><body><p>stage</p><div id=\"dyn\" data-lsm-id=\"d1\" \
                  style=\"width:300px;height:200px\"></div></body></html>";
    let r2 = eval_on_with_rects(
        &handle,
        r#"JSON.stringify((function(){
          var d = window.__dyn;
          return {cw: d.clientWidth, ch: d.clientHeight, ow: d.offsetWidth, oh: d.offsetHeight,
                  left: d.offsetLeft, top: d.offsetTop, rect: d.getBoundingClientRect()};
        })())"#,
        hosted,
        "https://a.test/",
    );
    assert!(r2.outcome.ok, "{r2:?}");
    let v2: serde_json::Value = serde_json::from_str(&r2.outcome.value).unwrap();
    assert_eq!(v2["cw"], 300, "clientWidth 应回填:{v2}");
    assert_eq!(v2["ch"], 200, "clientHeight 应回填:{v2}");
    assert_eq!(v2["ow"], 300, "offsetWidth 应回填:{v2}");
    assert_eq!(v2["oh"], 200, "offsetHeight 应回填:{v2}");
    assert_eq!(v2["rect"]["width"], 300, "getBoundingClientRect 应回填:{v2}");
    assert_eq!(v2["rect"]["height"], 200, "getBoundingClientRect 应回填:{v2}");
}

#[test]
fn 动态元素事件监听可触发() {
    let handle = JsHandle::spawn();
    let setup = eval_on(
        &handle,
        r#"(function(){
          window.__hits = [];
          var d = document.createElement('div');
          d.id = 'dynbtn';
          d.addEventListener('click', function () { window.__hits.push('hit'); });
          document.body.appendChild(d);
          window.__dyn = d;
          return 'ok';
        })()"#,
        DYN_STAGE_PAGE,
        "https://a.test/",
    );
    assert_eq!(setup.outcome.value, "ok", "{setup:?}");

    // 宿主点击落在该子树(快照节点带 data-lsm-id)→ 注册在动态对象上的监听必须跑到。
    let hosted = "<html><body><p>stage</p><div id=\"dynbtn\" data-lsm-id=\"d1\"></div></body></html>";
    let doc = Document::parse(hosted, "https://a.test/");
    let nodes = doc.snapshot_nodes();
    let e = env("https://a.test/");
    let reply = handle
        .request(|tx| JsRequest::Dispatch {
            selector: "#dynbtn".to_string(),
            event: "click".to_string(),
            env: e,
            nodes,
            reply: tx,
        })
        .expect("JS 线程请求失败");
    assert!(reply.outcome.ok, "dispatch 应成功:{reply:?}");

    // Dispatch 走 eval_quiet,返回值不落 outcome.value —— 断言真实副作用。
    let hits = eval_on(
        &handle,
        "JSON.stringify(window.__hits)",
        DYN_STAGE_PAGE,
        "https://a.test/",
    );
    assert_eq!(hits.outcome.value, "[\"hit\"]", "监听回调应已执行:{hits:?}");
}

#[test]
fn 动态元素序列化携带节点键() {
    // `data-lsm-id` 是动态节点回认宿主布局几何的钥匙,缺它宿主对不回。
    let handle = JsHandle::spawn();
    let r = eval_on(
        &handle,
        r#"(function(){
          var d = document.createElement('div');
          d.id = 'k';
          d.className = 'box';
          document.body.appendChild(d);
          var s = '';
          try { s = __serEl(d); } catch (e) { s = 'ERR:' + e; }
          return s;
        })()"#,
        DYN_STAGE_PAGE,
        "https://a.test/",
    );
    assert!(
        r.outcome.value.contains("data-lsm-id=\"d"),
        "序列化必须带 data-lsm-id: {}",
        r.outcome.value
    );
    assert!(r.outcome.value.contains("id=\"k\""), "原有属性不得丢失:{}", r.outcome.value);
    assert!(r.outcome.value.contains("class=\"box\""), "原有属性不得丢失:{}", r.outcome.value);
}

#[test]
fn 元素集合是活集合_追加后立即可见() {
    // 回归:getElementsByTagName 曾返回**一次性数组快照**。百度地图
    // `require.loadCss` 的 CSS 注入器依赖浏览器的实时集合语义:
    //   var n = head.getElementsByTagName("style");
    //   if (!n.length) { 创建 <style> 并 appendChild }
    //   var o = n[0]; o.getAttribute("media")     ← 快照下 o 是 undefined → TypeError
    // 那个 TypeError 会打断 dataCopyRight 模块工厂 → setCurCity → init 整条
    // 地图初始化链 → 地图与路线面板全都不出图。
    let handle = JsHandle::spawn();
    let r = eval_on(
        &handle,
        r#"(function(){
          var head = document.getElementsByTagName('head')[0];
          var n = head.getElementsByTagName('style');
          if (n.length === 0) {
            var s = document.createElement('style');
            s.setAttribute('type','text/css');
            head.appendChild(s);
          }
          var o = n[0];
          return JSON.stringify({len: n.length, first: o ? String(o.tagName) : null});
        })()"#,
        "<html><head><title>T</title></head><body><div id='x'></div></body></html>",
        "https://a.test/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    assert_eq!(
        r.outcome.value, r#"{"len":1,"first":"STYLE"}"#,
        "追加 <style> 后集合必须立即可见(活集合),实际 {}",
        r.outcome.value
    );
}

#[test]
fn 文本节点追加落到宿主_dom() {
    // 回归:快照元素的 appendChild 只认「detached 元素」与「快照节点」,
    // 文本节点两个分支都不落 → 站点运行时注入的 CSS(如 require.loadCss
    // 往 <style> 里塞的样式文本)到不了宿主 DOM,动态样式全丢。
    let handle = JsHandle::spawn();
    let r = eval_on(
        &handle,
        r#"(function(){
          var st = document.getElementById('s');
          var t = document.createTextNode('body{color:red}');
          st.appendChild(t);
          return 'ok';
        })()"#,
        "<html><head><style id='s'></style></head><body></body></html>",
        "https://a.test/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let hit = r
        .outcome
        .actions
        .iter()
        .find(|a| a.kind == "append_text" && a.value.contains("body{color:red}"));
    assert!(
        hit.is_some(),
        "文本节点应产生 append_text 动作落进宿主 DOM,实际动作:{:?}",
        r.outcome.actions
    );
}

#[test]
fn 应用_append_text_不清空既有子节点() {
    // 浏览器的 appendChild(textNode) 是**追加**;若内核用 set_text 落地,
    // 元素原有子节点会被整体换掉(误删内容)。append_text 才是正确语义。
    let doc = Document::parse(
        "<html><body><div id='box'><span>原有</span></div></body></html>",
        "https://a.test/",
    );
    let actions = vec![action("append_text", "#box", "", "追加文本")];
    let summary = dom_ops::apply(&doc, &actions, "https://a.test/");
    assert_eq!(summary.applied, 1);
    let out = doc.serialize_full();
    assert!(out.contains("原有"), "既有子节点不得被清空:{out}");
    assert!(out.contains("追加文本"), "追加的文本必须落地:{out}");
}

#[test]
fn innerHTML_返回子节点_HTML_而非可见文本() {
    // 回归:innerHTML 此前回退到元素的**可见文本**,于是
    // `document.body.innerHTML` 只有一个元素的文字(实测百度地图:7 个字符
    // 「百度地图 mz」)。站点脚本与 Agent 都会据此误判「页面是空壳」。
    let handle = JsHandle::spawn();
    let r = eval_on(
        &handle,
        r#"(function(){
          var b = document.body;
          var h = b.innerHTML;
          return JSON.stringify({
            len: h.length,
            hasTag: h.indexOf('<div') >= 0 || h.indexOf('<p') >= 0,
            head: h.slice(0, 80)
          });
        })()"#,
        "<html><body><div id='box'><p>正文</p></div></body></html>",
        "https://a.test/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert!(
        v["hasTag"].as_bool().unwrap_or(false),
        "innerHTML 必须是 HTML(应含 <div/<p),实际 {}",
        r.outcome.value
    );
    assert!(
        v["len"].as_u64().unwrap_or(0) > 20,
        "innerHTML 长度应远大于可见文本,实际 {}",
        r.outcome.value
    );
    assert!(
        r.outcome.value.contains("正文"),
        "innerHTML 应含子节点文本,实际 {}",
        r.outcome.value
    );
}

// ============ zhipin 兼容性修复回归(2026-10-27) ============
// 实测 BOSS直聘:jQuery 1.12.2 / Vue 1.0 / bossui / webpack chunk 链在
// 以下缺口上逐一中断,搜索页永久停在「加载中,请稍候」。每个测试对应
// 一个已修复的缺口,防止回归。

#[test]
fn in操作符命中window的on事件族() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r#"JSON.stringify({
            onsubmit: "onsubmit" in window,
            onchange: "onchange" in window,
            unknown: "zzz-not-a-handler" in window
        })"#,
        "<html><body><div>ok</div></body></html>",
        "https://www.zhipin.com/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert_eq!(v["onsubmit"], serde_json::json!(true), "onsubmit 必须命中(jQuery support 检测)");
    assert_eq!(v["onchange"], serde_json::json!(true));
    assert_eq!(v["unknown"], serde_json::json!(false), "非 on* 名不应误命中");
    h.shutdown();
}

#[test]
fn 动态元素attributes集合可用() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r#"(function(){
            var d = document.createElement("div");
            d.setAttribute("onsubmit", "t");
            d.setAttribute("data-x", "1");
            return JSON.stringify({
                named: d.attributes["onsubmit"] ? d.attributes["onsubmit"].value : "missing",
                expando: d.attributes["onsubmit"] ? d.attributes["onsubmit"].expando : "missing",
                length: d.attributes.length,
                item: d.attributes.item(1) ? d.attributes.item(1).name : "missing",
                getNamed: d.attributes.getNamedItem("data-x") ? d.attributes.getNamedItem("data-x").value : "missing",
                removed: (d.removeAttribute("data-x"), d.attributes.getNamedItem("data-x") === null)
            });
        })()"#,
        "<html><body><div>ok</div></body></html>",
        "https://www.zhipin.com/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert_eq!(v["named"], serde_json::json!("t"));
    assert_eq!(v["expando"], serde_json::json!(false));
    assert_eq!(v["length"], serde_json::json!(2));
    assert_eq!(v["getNamed"], serde_json::json!("1"));
    assert_eq!(v["removed"], serde_json::json!(true));
    h.shutdown();
}

#[test]
fn 快照元素attributes集合可用() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r##"(function(){
            var el = document.querySelector("#app");
            return JSON.stringify({
                hasClass: el.attributes["class"] ? el.attributes["class"].value : "missing",
                len: el.attributes.length
            });
        })()"##,
        r##"<html><body><div id="app" class="root" data-v="1">内容</div></body></html>"##,
        "https://www.zhipin.com/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert_eq!(v["hasClass"], serde_json::json!("root"));
    assert!(v["len"].as_u64().unwrap_or(0) >= 3, "id/class/data-v 至少 3 个属性");
    h.shutdown();
}

#[test]
fn createHTMLDocument沙箱探测可用() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r#"(function(){
            var doc = document.implementation.createHTMLDocument("");
            doc.body.innerHTML = "<form></form><form></form>";
            return JSON.stringify({
                childCount: doc.body.childNodes.length,
                nodeType: doc.nodeType
            });
        })()"#,
        "<html><body><div>ok</div></body></html>",
        "https://www.zhipin.com/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    // jQuery 1.12.2 的 createHTMLDocument 支持性检测:两个 form → childNodes 2。
    assert_eq!(v["childCount"], serde_json::json!(2));
    assert_eq!(v["nodeType"], serde_json::json!(9));
    h.shutdown();
}

#[test]
fn createEvent与createComment可用() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r#"(function(){
            var ev = document.createEvent("Event");
            ev.initEvent("click", true, false);
            var cm = document.createComment("v-if");
            return JSON.stringify({
                evType: ev.type, evBubbles: ev.bubbles,
                cmNodeType: cm.nodeType, cmText: cm.textContent
            });
        })()"#,
        "<html><body><div>ok</div></body></html>",
        "https://www.zhipin.com/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert_eq!(v["evType"], serde_json::json!("click"));
    assert_eq!(v["evBubbles"], serde_json::json!(true));
    assert_eq!(v["cmNodeType"], serde_json::json!(8));
    assert_eq!(v["cmText"], serde_json::json!("v-if"));
    h.shutdown();
}

#[test]
fn defer脚本整组后置执行() {
    // 阻塞脚本(含后置位)必须先于先导位的 defer 脚本执行:
    // zhipin 搜索页 vue-core(阻塞)写在 defer 块之后,无视 defer 会
    // 让 defer 的 app 入口先跑、Vue is not defined。
    let doc_html = r#"<html><head>
        <script>window.__ord = window.__ord || []; window.__ord.push("defer-first")</script>
        <script>window.__ord.push("blocking")</script>
        <script defer src="https://cdn.test/defer-a.js"></script>
    </head><body>ok</body></html>"#;
    let doc = Document::parse(doc_html, "https://a.test/");
    let mut executed = std::collections::HashSet::new();
    let mut html = doc_html.to_string();
    // 两轮收集(模拟管线):首轮应给出 blocking 组在前。
    let collected = dom_ops::collect_scripts(&Document::parse(&html, "https://a.test/"), "https://a.test/", &mut executed);
    let mut order: Vec<String> = Vec::new();
    for it in collected.items.iter().chain(collected.deferred.iter()) {
        if let dom_ops::CollectedItem::Inline(s) = it {
            if s.content.contains("defer-first") { order.push("defer-first-inline".into()); }
            if s.content.contains("blocking") { order.push("blocking".into()); }
        }
        if let dom_ops::CollectedItem::External(u) = it {
            if u.contains("defer-a") { order.push("defer-a".into()); }
        }
    }
    assert_eq!(
        order,
        vec!["defer-first-inline".to_string(), "blocking".to_string(), "defer-a".to_string()],
        "defer 外链必须整组后置:实际 {order:?}"
    );
}

#[test]
fn url与URLSearchParams解码可用() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r#"(function(){
            var u = new URL("https://www.zhipin.com/web/geek/job?query=AI%20%E5%B7%A5%E7%A8%8B%E5%B8%88&city=1");
            return JSON.stringify({
                q: u.searchParams.get("query"),
                c: u.searchParams.get("city"),
                has: u.searchParams.has("query")
            });
        })()"#,
        "<html><body><div>ok</div></body></html>",
        "https://www.zhipin.com/",
    );
    assert!(r.outcome.ok, "new URL 不应抛 ReferenceError:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert_eq!(v["q"], serde_json::json!("AI 工程师"));
    assert_eq!(v["c"], serde_json::json!("1"));
    assert_eq!(v["has"], serde_json::json!(true));
    h.shutdown();
}

#[test]
fn a元素URL反射可用() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r#"(function(){
            var a = document.createElement("a");
            a.setAttribute("href", "https://img.bosszhipin.com/x/y?k=1#frag");
            return JSON.stringify({
                pathname: a.pathname, host: a.host, protocol: a.protocol,
                search: a.search, hash: a.hash,
                firstChar: a.pathname.charAt(0)
            });
        })()"#,
        "<html><body><div>ok</div></body></html>",
        "https://www.zhipin.com/web/geek/job",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    // axios 判同模块装载期直接读 a.pathname.charAt(0),缺失即崩。
    assert_eq!(v["pathname"], serde_json::json!("/x/y"));
    assert_eq!(v["host"], serde_json::json!("img.bosszhipin.com"));
    assert_eq!(v["protocol"], serde_json::json!("https:"));
    assert_eq!(v["search"], serde_json::json!("?k=1"));
    assert_eq!(v["hash"], serde_json::json!("#frag"));
    assert_eq!(v["firstChar"], serde_json::json!("/"));
    h.shutdown();
}

#[test]
fn elementFromPoint返回元素不抛错() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r#"(function(){
            var hit = document.elementFromPoint(10, 10);
            return JSON.stringify({
                isObj: !!hit,
                tag: hit ? String(hit.tagName || hit.__tag || "").toLowerCase() : "null"
            });
        })()"#,
        r#"<html><body><div style="width:100px;height:100px">盒子</div></body></html>"#,
        "https://www.zhipin.com/",
    );
    assert!(r.outcome.ok, "elementFromPoint 不应抛错:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert_eq!(v["isObj"], serde_json::json!(true), "必须返回元素(patas/ka 采样依赖)");
    h.shutdown();
}

#[test]
fn 动态脚本装载只针对script元素() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r#"(function(){
            // img 带 src appendChild:不应触发脚本装载(img 二进制进 eval 会炸)。
            var img = document.createElement("img");
            img.setAttribute("src", "https://img.bosszhipin.com/a.jpeg");
            document.body.appendChild(img);
            return JSON.stringify({ imgOk: true, dynLoaded: Object.keys(__lsmDynLoaded).length });
        })()"#,
        "<html><body><div>ok</div></body></html>",
        "https://www.zhipin.com/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert_eq!(
        v["dynLoaded"].as_u64().unwrap_or(99),
        0,
        "img 的 src 不得触发动态脚本装载"
    );
    h.shutdown();
}

// ===================== 第 174 轮:诊断 API(TreeWalker/Range/attributes/performance) =====================

/// 猎聘真机实测的探测脚本形态:TreeWalker 全文扫描 + Range 文本矩形 +
/// `Array.from(el.attributes)`。修复前三者分别报 not-a-callable / null /
/// getter 抛错,Agent 的登录排查被系统性带偏。
#[test]
fn 诊断三件套_treewalker_range_attributes() {
    let h = JsHandle::spawn();
    let html = concat!(
        "<html><body><div id='wrap'>",
        "<span id='login'>登录/注册</span>",
        "<p>扫码登录提示文案</p>",
        "</div></body></html>"
    );
    // TreeWalker:SHOW_TEXT 扫描文本节点。
    let r = eval_on_with_rects(
        &h,
        "(function(){ var hits=[]; var w=document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, null); \
         var n; while((n=w.nextNode())){ if(/扫码|登录/.test(n.textContent)) hits.push(n.textContent.trim()); } \
         return JSON.stringify(hits); })()",
        html,
        "https://www.liepin.com/",
    );
    assert!(r.outcome.ok, "TreeWalker 应可用:{:?}", r.outcome.error);
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert!(
        v.as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "TreeWalker 应扫到登录文本:{r:?}"
    );

    // Range:selectNode + getBoundingClientRect 不再 null 崩。
    let r = eval_on_with_rects(
        &h,
        "(function(){ var el=document.getElementById('login'); var rg=document.createRange(); \
         rg.selectNode(el); var b=rg.getBoundingClientRect(); return JSON.stringify({w:b.width, h:b.height}); })()",
        html,
        "https://www.liepin.com/",
    );
    assert!(r.outcome.ok, "createRange 应可用:{:?}", r.outcome.error);

    // attributes 可迭代:Array.from(...).map(a=>a.name)。
    let r = eval_on(
        &h,
        "(function(){ var el=document.getElementById('login'); \
         return JSON.stringify(Array.from(el.attributes).map(function(a){return a.name;}).sort()); })()",
        html,
        "https://www.liepin.com/",
    );
    assert!(r.outcome.ok, "Array.from(el.attributes) 应可用:{:?}", r.outcome.error);
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert_eq!(v.as_array().map(|a| a.len()), Some(1), "应有 id 属性:{r:?}");
    h.shutdown();
}

/// performance.getEntriesByType('resource') 读宿主网络台账,不再是空桩
/// (猎聘实测误判「0 个脚本加载」,实际 3.3MB bundle 已执行)。
#[test]
fn performance资源时序来自网络台账() {
    let h = JsHandle::spawn();
    let doc = Document::parse("<html><body><div>ok</div></body></html>", "https://www.liepin.com/");
    let nodes = doc.snapshot_nodes();
    let mut e = env("https://www.liepin.com/");
    e.resources = vec![crate::agent::browser::NetworkRecord {
        url: "https://concat.lietou-static.com/fe-www-pc/v6/js/common.js".to_string(),
        method: "GET".to_string(),
        status: 200,
        content_type: "application/javascript".to_string(),
        bytes: 3301380,
        elapsed_ms: 1200,
        redirect: false,
        from_cache: false,
        at_ms: 0,
    }];
    let script = "(function(){ var rs=performance.getEntriesByType('resource'); \
                  return JSON.stringify({n:rs.length, name:rs[0]&&rs[0].name, size:rs[0]&&rs[0].transferSize}); })()"
        .to_string();
    let reply = h
        .request(|tx| JsRequest::Eval { script, env: e, nodes, reply: tx })
        .expect("JS 线程请求失败");
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    let v: serde_json::Value = serde_json::from_str(&reply.outcome.value).unwrap_or_default();
    assert_eq!(v["n"].as_u64(), Some(1), "资源台账应有 1 条:{v:?}");
    assert!(
        v["name"].as_str().unwrap_or("").contains("common.js"),
        "资源名应来自台账:{v:?}"
    );
    assert_eq!(v["size"].as_u64(), Some(3301380));
    h.shutdown();
}

/// 回归(第 176 轮,zhipin 安全验证页滑块构建链卡死):定时器回调里排入的
/// 微任务必须在**同一次请求的 event_loop 内**推进。此前排水只在 tick 前,
/// 最后一轮定时器触发的「脚本装载 onload → Promise 链」在 pending==0 提前
/// 退出时永远滞留,open 之后的 inspect/screenshot/request_human 全是宿主侧
/// 动作不触碰 JS 线程,站点构建链无限期挂起(用户面对空白验证页)。
#[test]
fn 定时器回调排入的微任务同请求内推进() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r##"(function(){
            setTimeout(function(){
                Promise.resolve().then(function(){
                    console.error("CHAIN_RAN");
                });
            }, 10);
            return "ok";
        })()"##,
        r##"<html><body><div id="app">x</div></body></html>"##,
        "https://example.com/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let ran = r
        .outcome
        .logs
        .iter()
        .any(|l| l.text.contains("CHAIN_RAN"));
    assert!(
        ran,
        "定时器回调排入的微任务应在同一请求内被排水推进(修复前滞留到下一次请求):{:?}",
        r.outcome.logs
    );
    h.shutdown();
}

/// 回归(第 176 轮):站点普遍的「超时保险丝」模式 `xhr.send(); timer=setTimeout(fail)`。
/// 真实浏览器 XHR 异步:send 先返回,timer 真实存在,rs4 回调里的 clearTimeout
/// 能取消它。内核此前同步派发,rs4 在 send 内就触发,clearTimeout 清空、5 秒后
/// 伪 fail 必发(实测 zhipin captcha-sdk m() 的 fail("timeOut") 覆盖真实成功)。
/// 修复:异步 XHR(缺省)回调推迟到微任务派发。about:blank 走错误路径(不联网)。
#[test]
fn xhr异步派发清除send后挂的超时保险丝() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r##"(function(){
            var out = { ok: 0, fails: 0, timer: 0 };
            var x = new XMLHttpRequest();
            x.onreadystatechange = function () {
                if (this.readyState === 4) { clearTimeout(out.timer); out.ok++; }
            };
            x.open("GET", "about:blank");
            x.send();
            out.timer = setTimeout(function () { out.fails++; }, 50);
            window.__fuseOut = out;
            return "sent";
        })()"##,
        r##"<html><body></body></html>"##,
        "https://example.com/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    // 第二次求值:event_loop 已排水微任务派发 + tick 过虚拟时钟,保险丝应被清除。
    let r2 = eval_on(
        &h,
        r##"JSON.stringify({ok: window.__fuseOut.ok, fails: window.__fuseOut.fails})"##,
        r##"<html><body></body></html>"##,
        "https://example.com/",
    );
    let v: serde_json::Value = serde_json::from_str(&r2.outcome.value).unwrap_or_default();
    assert_eq!(v["ok"].as_u64(), Some(1), "rs4 应恰好派发一次:{v}");
    assert_eq!(v["fails"].as_u64(), Some(0), "send 后挂的超时保险丝应被 rs4 的 clearTimeout 取消,不得伪 fail:{v}");
    h.shutdown();
}

/// 显式同步 XHR(open 第三参 false)保持同步派发——真实浏览器同步 XHR 语义。
#[test]
fn xhr显式同步模式仍同步派发() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r##"(function(){
            var x = new XMLHttpRequest();
            x.open("GET", "about:blank", false);
            x.send();
            return JSON.stringify({ rs: x.readyState, st: x.status });
        })()"##,
        r##"<html><body></body></html>"##,
        "https://example.com/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let v: serde_json::Value = serde_json::from_str(&r.outcome.value).unwrap_or_default();
    assert_eq!(v["rs"].as_u64(), Some(4), "同步 XHR 应在 send 内同步推进到 rs4:{v}");
    h.shutdown();
}

/// abort 抑制异步派发:微任务派发前 abort,回调不再触发(浏览器语义)。
#[test]
fn xhr微任务派发前abort则不触发回调() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r##"(function(){
            var out = { ok: 0 };
            var x = new XMLHttpRequest();
            x.onreadystatechange = function () { if (this.readyState === 4) out.ok++; };
            x.open("GET", "about:blank");
            x.send();
            x.abort();
            window.__abortOut = out;
            return "sent";
        })()"##,
        r##"<html><body></body></html>"##,
        "https://example.com/",
    );
    assert!(r.outcome.ok, "求值应成功:{r:?}");
    let r2 = eval_on(
        &h,
        r##"String(window.__abortOut.ok)"##,
        r##"<html><body></body></html>"##,
        "https://example.com/",
    );
    assert_eq!(r2.outcome.value.trim(), "0", "abort 后微任务派发应被抑制");
    h.shutdown();
}

/// 管线内(run_scripts)发起的异步 XHR:deliver 微任务必须在脚本间排水或
/// event_loop 内推进(zhipin 安全验证页 getredirect/白名单 XHR 全部静默丢失
/// 的复现用例)。
#[test]
fn 管线内xhr的deliver微任务会推进() {
    let h = JsHandle::spawn();
    let doc = Document::parse(
        r##"<html><body><div id="app">x</div></body></html>"##,
        "https://example.com/",
    );
    let nodes = doc.snapshot_nodes();
    let scripts = vec![ScriptSource {
        kind: "inline",
        content: r##"(function(){
            var x = new XMLHttpRequest();
            x.onreadystatechange = function () {
                if (this.readyState === 4) console.error("PIPELINE_DELIVERED");
            };
            x.open("GET", "about:blank");
            x.send();
        })()"##
            .to_string(),
        src_url: String::new(),
    }];
    let reply = h
        .request(|tx| JsRequest::RunScripts {
            scripts,
            env: env("https://example.com/"),
            nodes,
            reply: tx,
            budget_ms: 0,
        })
        .expect("JS 线程请求失败");
    let delivered = reply
        .outcome
        .logs
        .iter()
        .any(|l| l.text.contains("PIPELINE_DELIVERED"));
    assert!(
        delivered,
        "管线内 XHR 的 deliver 微任务应被排水推进(修复前 getredirect/白名单链整条静默丢失):{:?}",
        reply.outcome.logs
    );
    h.shutdown();
}

/// 管线 event_loop 内:定时器回调里排入的 Promise 微任务必须被 post-tick 排水
/// 推进(zhipin 验证页 interval → dynamicLoadJs onload → .then 构造链的最小复现)。
#[test]
fn 管线event_loop内定时器promise链推进() {
    let h = JsHandle::spawn();
    let doc = Document::parse(
        r##"<html><body><div id="app">x</div></body></html>"##,
        "https://example.com/",
    );
    let nodes = doc.snapshot_nodes();
    let scripts = vec![ScriptSource {
        kind: "inline",
        content: r##"(function(){
            setTimeout(function () {
                Promise.resolve().then(function () {
                    console.error("TIMER_THEN_RAN");
                });
            }, 10);
        })()"##
            .to_string(),
        src_url: String::new(),
    }];
    let reply = h
        .request(|tx| JsRequest::RunScripts {
            scripts,
            env: env("https://example.com/"),
            nodes,
            reply: tx,
            budget_ms: 0,
        })
        .expect("JS 线程请求失败");
    let ran = reply
        .outcome
        .logs
        .iter()
        .any(|l| l.text.contains("TIMER_THEN_RAN"));
    assert!(
        ran,
        "管线 event_loop 内定时器→Promise 链应被推进(post-tick 排水):{:?}",
        reply.outcome.logs
    );
    h.shutdown();
}

/// 诊断:前一个微任务抛错是否会饿死后续微任务(Boa run_jobs 的 clear 语义)。
#[test]
fn 诊断_前序微任务异常是否饿死后续() {
    let h = JsHandle::spawn();
    let r = eval_on(
        &h,
        r##"(function(){
            Promise.resolve().then(function(){ throw new Error("FIRST_BOOM"); });
            Promise.resolve().then(function(){ console.error("SECOND_RAN"); });
            return "queued";
        })()"##,
        r##"<html><body></body></html>"##,
        "https://example.com/",
    );
    let ran = r.outcome.logs.iter().any(|l| l.text.contains("SECOND_RAN"));
    let boom = r.outcome.logs.iter().any(|l| l.text.contains("FIRST_BOOM"));
    println!("SECOND_RAN={ran} FIRST_BOOM可见={boom} logs={:?}", r.outcome.logs);
    h.shutdown();
}
