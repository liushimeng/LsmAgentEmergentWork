//! 京东首屏脚本链兼容性回归单测(第 47 轮真机根因)。
//!
//! 背景(2026-10-30 京东实测):`MCP_Web_LsmAgentBrowser_Use` 打开
//! `https://www.jd.com`,页面只剩服务器静态壳(登录栏/轮播/楼层全无),
//! 随即被风控换「安全验证」滑块页。逐层定位出四个 prelude/env_js 缺口,
//! 全部集中在 jQuery 1.6.4 → wl.js(风控指纹) → js_security_v3 三段链:
//!
//! 1. `DocumentFragment` 是只有 appendChild 的裸对象:jQuery support 探测
//!    `l.cloneNode(!0).cloneNode(!0).lastChild.checked` 当场
//!    "not a callable function",jQuery 整体装载失败 → 首页动态内容全灭;
//! 2. `__newEl` 动态节点缺 `matches/matchesSelector/closest`:Sizzle 把快照
//!    元素的 matchesSelector 当原生实现收编,`b.call(createElement("div"),
//!    "div")` 探测时 `this.matches` 为 undefined → 同样断链;
//! 3. `navigator.javaEnabled` 缺失:wl.js `_e` 指纹采集
//!    `t&&t.javaEnabled()?1:0` 不带守卫直接调用;
//! 4. `RegExp.$1` legacy 静态属性:Boa 0.22 未实现,js_security_v3 的老式
//!    日期格式化 `/(y+)/i.test(s)` → `RegExp.$1.length` 抛
//!    "cannot convert 'null' or 'undefined' to object"(Boa 该文案与读
//!    undefined 属性共用)。
//!
//! 本文件用京东真实脚本的**同构最小片段**锁死以上四个缺口;不引入网络与
//! 真站依赖,毫秒级跑完。

use super::tests::{env, eval_on};
use super::JsHandle;

const PAGE: &str = "<html><body><div id='x'>hi</div></body></html>";

fn eval_str(handle: &JsHandle, script: &str) -> String {
    let reply = eval_on(handle, script, PAGE, "https://www.jd.com/");
    assert!(
        reply.outcome.ok,
        "求值失败: {:?} — 脚本: {script}",
        reply.outcome.error
    );
    reply.outcome.value
}

#[test]
fn fragment_完整节点接口与克隆链() {
    let h = JsHandle::spawn();
    // jQuery 1.6.4 support 探测原句的最小同构:
    // createDocumentFragment → appendChild(firstChild) → cloneNode×2 → lastChild
    let v = eval_str(
        &h,
        r#"(function(){
            var a = document.createElement("div");
            a.innerHTML = "<input type='checkbox' checked/>";
            var l = document.createDocumentFragment();
            l.appendChild(a.firstChild);
            var cloned = l.cloneNode(true).cloneNode(true);
            return [
                "frag_nt=" + l.nodeType,
                "clone_nt=" + cloned.nodeType,
                "lastChild_checked=" + (cloned.lastChild ? cloned.lastChild.checked : "NONE"),
                "childNodes=" + l.childNodes.length,
                "firstChild=" + (l.firstChild ? "Y" : "N")
            ].join(" ");
        })()"#,
    );
    assert!(v.contains("frag_nt=11"), "fragment nodeType 应为 11: {v}");
    assert!(v.contains("clone_nt=11"), "克隆的克隆也应保持 11: {v}");
    // .checked 反射未建模属既有边界(读出 undefined,jQuery 对 falsy 有优雅
    // 降级);此处锁死的是「克隆链能走到 lastChild 且求值不抛」。
    assert!(!v.contains("lastChild_checked=NONE"), "克隆链应能取到 lastChild: {v}");
    assert!(v.contains("childNodes=1"), "fragment childNodes 应可见: {v}");
    assert!(v.contains("firstChild=Y"), "fragment firstChild 应可见: {v}");
    h.shutdown();
}

#[test]
fn 动态节点_matches_家族可调用() {
    let h = JsHandle::spawn();
    // Sizzle 收编快照 matchesSelector 后用 .call(detachedDiv, "div") 探测:
    // 动态节点的 this.matches 必须可调用。
    let v = eval_str(
        &h,
        r#"(function(){
            var native = document.documentElement.matchesSelector;
            var probe = document.createElement("div");
            var ok = true;
            try { native.call(probe, "div"); } catch (e) { ok = false; }
            return [
                "cross_call=" + ok,
                "self_match=" + probe.matches("div"),
                "sel_alias=" + probe.matchesSelector("div"),
                "closest=" + (probe.closest("div") === probe ? "self" : "other")
            ].join(" ");
        })()"#,
    );
    assert!(v.contains("cross_call=true"), "快照 matchesSelector call 到动态节点上必须可用: {v}");
    assert!(v.contains("self_match=true"), "动态节点自身 matches 应命中标签: {v}");
    assert!(v.contains("sel_alias=true"), "matchesSelector 别名应等价: {v}");
    assert!(v.contains("closest=self"), "closest 应命中自身: {v}");
    h.shutdown();
}

#[test]
fn navigator_java_enabled_可直接调用() {
    let h = JsHandle::spawn();
    let v = eval_str(&h, r#"(function(){ var t = navigator; return "javaEnabled=" + (t && t.javaEnabled() ? 1 : 0); })()"#);
    assert!(v.contains("javaEnabled=0"), "wl.js 式 t&&t.javaEnabled()?1:0 必须不抛: {v}");
    h.shutdown();
}

#[test]
fn audio_构造器存在且可实例化() {
    let h = JsHandle::spawn();
    let v = eval_str(&h, r#"(function(){ var a = new Audio("x.mp3"); return "paused=" + a.paused + " canPlay=" + (a.canPlayType("mp3") === ""); })()"#);
    assert!(v.contains("paused=true"), "new Audio 应可用: {v}");
    h.shutdown();
}

#[test]
fn regexp_legacy_静态属性_记录捕获组() {
    let h = JsHandle::spawn();
    // js_security_v3 老式日期格式化路径:test → 读 RegExp.$1.length
    let v = eval_str(
        &h,
        r#"(function(){
            var fk = "yyyy-MM-dd";
            if (/(y+)/i.test(fk)) {
                fk = fk.replace(RegExp.$1, String(2026).substr(0, RegExp.$1.length));
            }
            return "fmt=" + fk + " dollar2=" + RegExp.$2;
        })()"#,
    );
    assert!(v.contains("fmt=2026-MM-dd"), "RegExp.$1 应记录捕获组并参与格式化: {v}");
    h.shutdown();
}

#[test]
fn jquery_support_探测链_最小同构不抛() {
    let h = JsHandle::spawn();
    // jQuery 1.6.4 `f.support` 探测块开头的整段逗号链(截取真实源码形态):
    // createElement → innerHTML 探测 → fragment → cloneNode 链 → 末尾 matches
    // 收编探测。任何一环缺口都会让 jQuery 装载失败。
    let v = eval_str(
        &h,
        r#"(function(){
            var c = document;
            var a = c.createElement("div");
            a.setAttribute("className", "t");
            a.innerHTML = "   <link/><table></table><a href='/a' style='top:1px;float:left;opacity:.55;'>a</a><input type='checkbox'/>";
            var d = a.getElementsByTagName("*"), e = a.getElementsByTagName("a")[0];
            if (!d || !d.length || !e) return "EARLY_RETURN";
            var i = c.createElement("input");
            i.setAttribute("type", "checkbox");
            i.setAttribute("checked", "checked");
            a.appendChild(i);
            var l = c.createDocumentFragment();
            l.appendChild(a.firstChild);
            var checkClone = l.cloneNode(!0).cloneNode(!0).lastChild.checked;
            a.innerHTML = "";
            a.style.width = a.style.paddingLeft = "1px";
            var m = c.getElementsByTagName("body")[0];
            var o = c.createElement(m ? "div" : "body");
            var nativeMatches = document.documentElement.matchesSelector;
            var cross = true;
            try { nativeMatches.call(o, "div"); } catch (ex) { cross = false; }
            return "checkClone=" + checkClone + " cross_matches=" + cross;
        })()"#,
    );
    assert!(
        !v.contains("EARLY_RETURN"),
        "detached innerHTML 探测应能取到子元素(否则 support 提前空返回): {v}"
    );
    assert!(v.contains("cross_matches=true"), "Sizzle matchesSelector 收编探测必须可用: {v}");
    h.shutdown();
}

// ===================== 京东 html 能力探测类名回写(2026-10-30 真机) =====================
// 首页 inline script[7] 原句:`document.getElementsByTagName("html")[0].className =
// "root61 o2_wide"`(视口≥1370 探测)。真机 dump 里 `<html>` 无类名 →
// `html.new_version`/`body.better_style` 限定的几十条 CSS 全不生效
// (搜索栏 form 的 calc 宽度、fs_col 尺寸全错)。锁死「根元素 className
// 写入能生成 set_attr 动作并落到序列化 HTML」。

#[test]
fn 京东html能力探测类名写入回传宿主() {
    use crate::agent::browser::document::Document;
    use super::dom_ops;

    let h = JsHandle::spawn();
    let page = "<html><head></head><body class='index'><div id='x'>hi</div></body></html>";
    // script[6] 同构:定义 pageConfig 全局。
    eval_str(&h, "window.pageConfig = {};");
    // script[7] 原句 + 收尾探针:把关键中间值带回,便于断言自解释。
    let reply = eval_on(
        &h,
        r#"!function(e){pageConfig.isWide=function(){var n=e,i=document,o=i.documentElement,t=i.getElementsByTagName("body")[0],a=n.innerWidth||o.clientWidth||t.clientWidth;return a>=1370}();var n=[];pageConfig.isWide?(n.push("root61"),n.push("o2_wide")):n.push("o2_mini");var i=document.getElementsByTagName("html")[0];i.className=n.join(" ")}(window,void 0);"#,
        page,
        "https://www.jd.com/",
    );
    assert!(reply.outcome.ok, "script[7] 同构不应抛错: {:?}", reply.outcome.error);
    let has_action = reply.outcome.actions.iter().any(|a| {
        a.kind == "set_attr" && a.key == "class" && a.selector.contains("html")
    });
    // 动作应携带类名回传(不要求具体档位:o2_wide/o2_mini 都算写入成功)。
    let class_action = reply.outcome.actions.iter().find(|a| a.kind == "set_attr" && a.key == "class");
    assert!(
        has_action,
        "className 写入应生成 set_attr(class) 动作(selector 命中 html): {:?}",
        reply.outcome.actions.iter().map(|a| format!("{}:{}", a.kind, a.selector)).collect::<Vec<_>>()
    );
    let value = class_action.map(|a| a.value.clone()).unwrap_or_default();
    assert!(
        value.contains("o2_"),
        "类名应含能力档位(o2_wide/o2_mini): {value}"
    );
    // 动作落到宿主序列化 HTML。
    let doc = Document::parse(page, "https://www.jd.com/");
    dom_ops::apply(&doc, &reply.outcome.actions, "https://www.jd.com/");
    let html = doc.serialize_full();
    assert!(html.contains("o2_"), "序列化 HTML 的 html 标签应带上能力类名: {html}");
    h.shutdown();
}

#[test]
fn body类名写入回传宿主_京东fillRootClass路径() {
    use crate::agent::browser::document::Document;
    use super::dom_ops;

    let h = JsHandle::spawn();
    let page = "<html><head></head><body class='index'><div id='x'>hi</div></body></html>";
    // index.chunk.js fillRootClass 同构:`el.className` 读改写(addClass/toggleClass 内部形态)。
    let reply = eval_on(
        &h,
        r#"(function(){
            var el = document.body;
            var cls = String(el.className || "");
            el.className = cls + " o2_mac new_version better_style";
            return el.className;
        })()"#,
        page,
        "https://www.jd.com/",
    );
    assert!(reply.outcome.ok, "body className 读改写不应抛错: {:?}", reply.outcome.error);
    assert!(
        reply.outcome.value.contains("better_style"),
        "读回应含新类名: {}",
        reply.outcome.value
    );
    let class_action = reply.outcome.actions.iter().find(|a| a.kind == "set_attr" && a.key == "class");
    assert!(class_action.is_some(), "应生成 set_attr(class) 动作: {:?}", reply.outcome.actions);
    let doc = Document::parse(page, "https://www.jd.com/");
    dom_ops::apply(&doc, &reply.outcome.actions, "https://www.jd.com/");
    let html = doc.serialize_full();
    assert!(
        html.contains("better_style"),
        "序列化 HTML 的 body 应带上新类名: {html}"
    );
    h.shutdown();
}

#[test]
fn body_classList_add写入回传宿主() {
    use crate::agent::browser::document::Document;
    use super::dom_ops;

    let h = JsHandle::spawn();
    let page = "<html><head></head><body class='index'><div id='x'>hi</div></body></html>";
    // index.chunk.js 同构:`document.body.classList.add("new_version")`(fillRootClass
    // 之后的类名设置原句形态)。锁死 classList.add → set_attr(class) 回传链。
    let reply = eval_on(
        &h,
        r#"(function(){
            document.body.classList.add("new_version");
            document.body.classList.add("better_style");
            return document.body.className;
        })()"#,
        page,
        "https://www.jd.com/",
    );
    assert!(reply.outcome.ok, "classList.add 不应抛错: {:?}", reply.outcome.error);
    assert!(
        reply.outcome.value.contains("better_style") && reply.outcome.value.contains("new_version"),
        "className 读回应含新类名: {}",
        reply.outcome.value
    );
    let class_action = reply.outcome.actions.iter().find(|a| a.kind == "set_attr" && a.key == "class");
    assert!(class_action.is_some(), "应生成 set_attr(class) 动作: {:?}", reply.outcome.actions);
    let doc = Document::parse(page, "https://www.jd.com/");
    dom_ops::apply(&doc, &reply.outcome.actions, "https://www.jd.com/");
    let html = doc.serialize_full();
    assert!(
        html.contains("new_version") && html.contains("better_style"),
        "序列化 HTML 的 body 应带上新类名: {html}"
    );
    h.shutdown();
}

// ===================== 管线超时中断语义(2026-10-30 京东) =====================
// 预算耗尽不再静默弃跑:超时轮次照常记账(防双执行)、按截断上报,
// 补泵轮把线程迟到的动作经回复邮箱合并回流,页面从「半成品」收敛到完成态。

#[tokio::test]
async fn 管线超时中断后补泵收尾且不双执行() {
    use crate::agent::browser::LsmBrowserManager;
    use super::{run_pipeline, PipelineSeed};

    // 宽限收到 300ms:用例墙钟不依赖默认 60s 宽限。
    std::env::set_var("LAEW_WEB_SCRIPT_GRACE_MS", "300");
    let mgr = LsmBrowserManager::new();
    let html = r#"<html><body><div id='x'>base</div><script>
window.__n = (window.__n || 0) + 1;
var t0 = Date.now(); while (Date.now() - t0 < 5000) {}
document.getElementById('x').textContent = 'heavy-ran';
</script></body></html>"#;
    let session = std::collections::HashMap::new();
    let rects: std::collections::HashMap<String, (f64, f64, f64, f64)> = std::collections::HashMap::new();
    // 轮1:300ms 预算(内部 +2s slack)+300ms 宽限 < 5s 忙等 → 超时中断。
    let out1 = {
        let mut inner = mgr.inner.lock().await;
        run_pipeline(
            &mut inner,
            &mgr.http,
            html,
            "https://t.test/",
            "ua",
            (1280, 720),
            &session,
            None,
            &rects,
            PipelineSeed { budget_ms: 300, ..Default::default() },
        )
        .await
    };
    assert!(
        out1.budget_cut,
        "超时中断应按截断上报(pending={})",
        out1.pending_scripts
    );
    // 轮2(补泵语义):同线程大预算 → 空轮排空,把 heavy 的迟到动作合并回来。
    let out2 = {
        let mut inner = mgr.inner.lock().await;
        run_pipeline(
            &mut inner,
            &mgr.http,
            &out1.html,
            "https://t.test/",
            "ua",
            (1280, 720),
            &session,
            None,
            &rects,
            PipelineSeed {
                handle: out1.js.clone(),
                executed: out1.executed.clone(),
                budget_ms: 15_000,
                ..Default::default()
            },
        )
        .await
    };
    assert!(
        out2.html.contains("heavy-ran"),
        "补泵应把线程迟到的 DOM 动作合并回流"
    );
    assert_eq!(out2.pending_scripts, 0, "补泵后应收敛");
    // 不双执行:heavy 的窗口计数只 +1。
    let h = out2.js.clone().expect("句柄应在");
    let v = eval_on(&h, "String(window.__n)", &out2.html, "https://t.test/");
    assert_eq!(v.outcome.value, "1", "超时轮次脚本不得重跑");
    h.shutdown();
}
