//! P2e 人类键鼠单测:边界事件不冒泡派发 / 多目标派发 / select 读值桥 /
//! pointer capture 三件套(39 号设计文档 P2e,57 号文档)。
//!
//! `tests.rs` 已超 1800 行规范,按仓库约定新用例放同级兄弟文件。

use super::tests::{eval_on, run_scripts};
use super::{JsAction, JsHandle};

// ---------- P2e:边界事件不冒泡 / 多目标派发 / select 读值桥 / pointer capture ----------

const P2E_PAGE: &str = "<html><head><title>页</title></head><body>\
<div id='wrap'><a id='go' href='https://a.test/x'>去</a><span id='side'>旁</span></div>\
<select id='s'><option value='1'>One</option><option value='2' selected>Two</option></select>\
</body></html>";

#[test]
fn 边界事件不冒泡_只命中自身监听() {
    let handle = JsHandle::spawn();
    // 祖先 #wrap 与目标 #go 各挂 mouseenter:不冒泡派发到 #go 时只应触发 #go。
    let reply = run_scripts(
        &handle,
        vec![
            "window.__seq = '';",
            "document.getElementById('wrap').addEventListener('mouseenter', function(){ window.__seq += 'W'; });",
            "document.getElementById('wrap').addEventListener('mouseover', function(){ window.__seq += 'W'; });",
            "document.getElementById('go').addEventListener('mouseenter', function(){ window.__seq += 'G'; });",
        ],
        P2E_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{reply:?}");
    let out = eval_on(&handle, "__lsmDispatchMulti([[\"#go\",\"mouseenter\",null,1]]); window.__seq", P2E_PAGE, "https://a.test/");
    assert_eq!(out.outcome.value, "G", "不冒泡:祖先监听不应触发:{out:?}");
    // 对照:冒泡位(0)走 __fire,祖先链监听命中(#go 无 mouseover 监听,
    // 事件沿链上行触发 #wrap 的监听 —— 既有 __fire 语义不回归)。
    let out = eval_on(&handle, "window.__seq=''; __lsmDispatchMulti([[\"#go\",\"mouseover\",null,0]]); window.__seq", P2E_PAGE, "https://a.test/");
    assert_eq!(out.outcome.value, "W", "冒泡:祖先监听应触发:{out:?}");
    handle.shutdown();
}

#[test]
fn 多目标派发_按传入顺序逐目标执行() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![
            "window.__seq = '';",
            "document.getElementById('wrap').addEventListener('mouseleave', function(){ window.__seq += 'LW'; });",
            "document.getElementById('go').addEventListener('pointerleave', function(){ window.__seq += 'PG'; });",
            "document.getElementById('side').addEventListener('pointerover', function(){ window.__seq += 'OS'; });",
        ],
        P2E_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{reply:?}");
    let out = eval_on(
        &handle,
        "__lsmDispatchMulti([[\"#go\",\"pointerleave\",null,1],[\"#wrap\",\"mouseleave\",null,1],[\"#side\",\"pointerover\",null,0]]); window.__seq",
        P2E_PAGE,
        "https://a.test/",
    );
    assert_eq!(out.outcome.value, "PGLWOS", "逐事件目标与顺序钉死:{out:?}");
    handle.shutdown();
}

#[test]
fn select读值桥_value与selectedIndex可读可写() {
    let handle = JsHandle::spawn();
    let out = eval_on(
        &handle,
        "var s = document.getElementById('s'); s.value + '/' + s.selectedIndex + '/' + s.options.length + '/' + s.selectedOptions[0].value",
        P2E_PAGE,
        "https://a.test/",
    );
    assert_eq!(out.outcome.value, "2/1/2/2", "select.value/selectedIndex/options/selectedOptions:{out:?}");
    // setter:selectedIndex=0 → set_value "1" 回写宿主(经 form_values 物化 selected)。
    let out = eval_on(
        &handle,
        "var s = document.getElementById('s'); s.selectedIndex = 0; 'ok'",
        P2E_PAGE,
        "https://a.test/",
    );
    assert_eq!(out.outcome.value, "ok");
    let hit = out
        .outcome
        .actions
        .iter()
        .find(|a| a.kind == "set_value" && a.selector.contains("s"));
    assert!(hit.is_some(), "selectedIndex setter 应回写 set_value:{:?}", out.outcome.actions);
    assert_eq!(hit.unwrap().value, "1");
    // option.selected 读路径。
    let out = eval_on(
        &handle,
        "var s = document.getElementById('s'); s.options[1].selected + '/' + s.options[0].selected",
        P2E_PAGE,
        "https://a.test/",
    );
    assert_eq!(out.outcome.value, "true/false", "option.selected:{out:?}");
    handle.shutdown();
}

#[test]
fn setPointerCapture回写宿主动作_hasPointerCapture读宿主态() {
    let handle = JsHandle::spawn();
    let out = eval_on(
        &handle,
        "var a = document.getElementById('go'); a.setPointerCapture(1); a.hasPointerCapture(1)",
        P2E_PAGE,
        "https://a.test/",
    );
    assert_eq!(out.outcome.value, "false", "宿主未注入捕获态(测试 env 为空)时 hasPointerCapture=false:{out:?}");
    let set: Vec<&super::JsAction> = out
        .outcome
        .actions
        .iter()
        .filter(|a| a.kind == "pointer_capture")
        .collect();
    assert_eq!(set.len(), 1, "应回写一个 pointer_capture 动作:{:?}", out.outcome.actions);
    assert_eq!(set[0].key, "set");
    assert!(set[0].selector.contains("go"));
    // releasePointerCapture 同路径。
    let out = eval_on(
        &handle,
        "var a = document.getElementById('go'); a.releasePointerCapture(1); 'ok'",
        P2E_PAGE,
        "https://a.test/",
    );
    assert!(out
        .outcome
        .actions
        .iter()
        .any(|a| a.kind == "pointer_capture" && a.key == "release"), "{:?}", out.outcome.actions);
    handle.shutdown();
}

