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
