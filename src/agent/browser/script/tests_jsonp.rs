//! JSONP 回调时序与 window 全局绑定回归单测(2026-11-01 天猫真机根因)。
//!
//! 背景:天猫首页 mtop JSONP(`callback=mtopjsonptbpc1`)在内核里报
//! `ReferenceError mtopjsonptbpc1 is not defined`,楼层/导航数据链整条断掉,
//! 首屏只剩 SSR + 静态壳。两处根因:
//!
//! 1. **eager 执行时序**:动态 `<script src>` 为 zhipin chunk 装载改成了
//!    「注册即同步取回执行」,而 JSONP 调用方常在 `appendChild` **之后**才注册
//!    回调 —— 浏览器里脚本异步执行天然错开该时序,eager 内核必炸;
//! 2. **window→全局同步丢更新**:Proxy `set` 陷阱旧条件 `!(k in globalThis)`,
//!    同名回调再注册时全局绑定滞留旧值(浏览器里 `window.foo = fn` 必须覆盖)。
//!
//! 本文件锁死:ReferenceError 延迟重试(≤3 次)最终送达回调、重试耗尽才走
//! 错误路径、window 再注册覆盖全局绑定、delete 同步清绑定。不引入网络依赖。

use super::tests::{eval_on, run_scripts};
use super::JsHandle;

const PAGE: &str = "<html><head></head><body></body></html>";

fn eval_str(handle: &JsHandle, script: &str) -> String {
    let reply = eval_on(handle, script, PAGE, "https://a.test/");
    assert!(
        reply.outcome.ok,
        "求值失败: {:?} — 脚本: {script}",
        reply.outcome.error
    );
    reply.outcome.value
}

/// 场景一(真机主根因):JSONP 回调在 appendChild **之后**注册。
/// eager 执行抛 ReferenceError 后必须延迟重试,回调最终收到数据。
#[test]
fn jsonp回调晚注册_延迟重试后数据送达() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
            globalThis.__lsmFetchNative = function (url, opts) {
              return JSON.stringify({ ok: true, status: 200, body: "cb({\"ok\":1})" });
            };
            window.__got = "none";
            var s = document.createElement('script');
            s.src = 'https://api.test/jsonp?callback=cb';
            document.head.appendChild(s);
            // 故意在 appendChild 之后才注册回调(实测 mtop SDK 时序)
            window.cb = function (data) { window.__got = "got:" + data.ok; };
        "#],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "页面脚本不得抛错:{:?}", reply.outcome.error);
    // 推进虚拟时钟:重试挂在 setTimeout(0)
    let got = eval_str(&handle, "(function(){ __lsmTick(); return String(window.__got); })()");
    assert_eq!(got, "got:1", "延迟重试必须把 JSONP 数据送达回调");
    handle.shutdown();
}

/// 场景二:回调始终未注册(真断链)。重试耗尽后不许死循环,
/// 错误必须如实入账(动态脚本执行失败),且后续求值照常可用(不悬挂)。
#[test]
fn jsonp回调始终缺失_重试耗尽后如实报错不悬挂() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
            globalThis.__lsmFetchNative = function (url, opts) {
              return JSON.stringify({ ok: true, status: 200, body: "neverDefinedCb({\"ok\":1})" });
            };
            window.__got = "none";
            var s = document.createElement('script');
            s.src = 'https://api.test/jsonp_missing';
            document.head.appendChild(s);
        "#],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "页面脚本不得抛错:{:?}", reply.outcome.error);
    // RunScripts 管线自带事件循环排空:延迟重试链已在本次请求内跑完收口,
    // 错误记录随本请求的 console 返回(后续 eval 会取走台账,须在此断言)。
    let logged_error = reply
        .outcome
        .logs
        .iter()
        .any(|l| l.text.contains("动态脚本执行失败") && l.text.contains("neverDefinedCb"));
    assert!(
        logged_error,
        "绑定缺失必须如实入账:{:?}",
        reply.outcome.logs
    );
    // 收口后不得悬挂:后续求值照常可用,重试队列已清空
    let state = eval_str(
        &handle,
        r#"(function(){
            for (var i = 0; i < 4; i++) { __lsmTick(); }
            return "q=" + __lsmDynRetryQueue.length + " pending=" + JSON.stringify(__lsmDynPending);
        })()"#,
    );
    assert_eq!(state, "q=0 pending={}", "重试队列须收敛清空: {state}");
    handle.shutdown();
}

/// 场景三:window 同名回调再注册必须覆盖全局绑定(浏览器全局对象语义)。
#[test]
fn window再注册同名回调覆盖全局绑定() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
            window.cb = function () { return "first"; };
            window.cb = function () { return "second"; };
            window.__viaBare = (typeof cb === "function") ? cb() : "MISSING";
        "#],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "页面脚本不得抛错:{:?}", reply.outcome.error);
    let v = eval_str(&handle, "String(window.__viaBare)");
    assert_eq!(v, "second", "裸 cb 调用必须见到最新注册的实现");
    handle.shutdown();
}

/// 场景四:delete window.foo 同步清全局绑定(裸标识符随之消失)。
#[test]
fn window删除回调同步清全局绑定() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
            window.cb = function () { return "x"; };
            delete window.cb;
            window.__afterDelete = (typeof cb === "undefined") ? "cleared" : "stale";
        "#],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "页面脚本不得抛错:{:?}", reply.outcome.error);
    let v = eval_str(&handle, "String(window.__afterDelete)");
    assert_eq!(v, "cleared", "delete window.foo 之后裸 foo 必须不可见");
    handle.shutdown();
}

/// 场景五:`script.text = code` 注入(PutForwards=textContent 语义)——
/// 站点内联脚本注入的经典写法。缺访问器会把代码写进死属性;script 是
/// raw-text 元素,序列化 HTML 转义会把 `&&`/`<` 转义坏,宿主取回执行必炸。
#[test]
fn script_text属性注入的正文原样进动作不被转义() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
            var s = document.createElement('script');
            s.text = "window.__injected = (1 && 2) < 10 ? 'ok' : 'bad';";
            document.body.appendChild(s);
        "#],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "页面脚本不得抛错:{:?}", reply.outcome.error);
    let joined: String = reply
        .outcome
        .actions
        .iter()
        .map(|a| format!("{}|{}|{}", a.kind, a.key, a.value))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("window.__injected = (1 && 2) < 10"),
        "script.text 正文必须原样序列化进 DOM 动作: {joined}"
    );
    assert!(
        !joined.contains("&amp;&amp;") && !joined.contains("&lt;10"),
        "raw-text 元素正文不得 HTML 转义: {joined}"
    );
    handle.shutdown();
}
