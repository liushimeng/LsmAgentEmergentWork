//! insertAdjacentHTML 顶层节点直接入子(co 模式)与 style 驼峰归一化单测。
//!
//! 背景(第 176 轮真机):百度地图 `co(t,e){return t.insertAdjacentHTML
//! ("beforeEnd",e),t.lastChild}` 建瓦片原点层。旧实现把 HTML 裹进伪元素壳,
//! `lastChild` 取到壳 → 页面把 `display/left/top` 写到无 position 的壳上 →
//! 瓦片坐标系原点塌到 (0,0),栅格整体偏移、下半屏空白。另有 `style.zIndex`
//! 驼峰名原样序列化导致宿主 CSS 解析器不识别的独立根因。

use super::tests::{env, eval_on, DYN_STAGE_PAGE};
use super::JsHandle;

// ---------- co 模式:快照路径 ----------

#[test]
fn co模式_快照路径_lastChild取到真元素() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"(function(){
          var p = document.getElementById('stage');
          p.insertAdjacentHTML('beforeend', '<div id="L" style="position:absolute;z-index:0"></div>');
          var o = p.lastChild;
          o.style.display = '';
          o.style.left = '960px';
          o.style.top = '540px';
          return JSON.stringify({
            tag: o.tagName, id: o.id,
            style: o.getAttribute('style'),
            kids: p.children.length
          });
        })()"#,
        r#"<html><body><div id="stage"><i>keep</i></div></body></html>"#,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    let v: serde_json::Value = serde_json::from_str(&reply.outcome.value).unwrap();
    assert_eq!(v["tag"], "DIV", "{v}");
    assert_eq!(v["id"], "L", "lastChild 必须是真正插入的元素而非伪元素壳:{v}");
    assert_eq!(v["kids"], 2, "顶层节点直接入子,不额外加壳:{v}");
    let style = v["style"].as_str().unwrap();
    assert!(style.contains("position:absolute"), "position 必须保留在真元素上:{style}");
    assert!(style.contains("left:960px"), "{style}");

    // 宿主动作:append_html 的 value 就是插入元素本身(单层,无壳嵌套)。
    let app = reply
        .outcome
        .actions
        .iter()
        .find(|a| a.kind == "append_html" && a.selector.contains("stage"))
        .expect("应有 append_html");
    assert!(app.value.contains(r#"id="L""#), "{}", app.value);
    assert!(app.value.contains("position:absolute"), "{}", app.value);
    assert_eq!(app.value.matches("<div").count(), 1, "不应有壳包裹:{}", app.value);
}

// ---------- co 模式:挂载后的动态元素路径(百度地图 platform 实况) ----------

#[test]
fn co模式_动态路径_lastChild取到真元素() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"(function(){
          var platform = document.createElement('div');
          platform.id = 'platform';
          document.body.appendChild(platform);
          // co 辅助函数原样回放:insertAdjacentHTML + lastChild
          var o = (platform.insertAdjacentHTML('beforeend',
              '<div style="position:absolute;z-index:0"></div>'), platform.lastChild);
          o.style.display = '';
          o.style.left = '960px';
          o.style.top = '540px';
          return JSON.stringify({
            tag: o.tagName,
            style: o.getAttribute('style'),
            kids: platform.children.length
          });
        })()"#,
        DYN_STAGE_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    let v: serde_json::Value = serde_json::from_str(&reply.outcome.value).unwrap();
    assert_eq!(v["tag"], "DIV", "{v}");
    assert_eq!(v["kids"], 1, "顶层节点直接入子,不裹壳:{v}");
    let style = v["style"].as_str().unwrap();
    assert!(style.contains("position:absolute"), "{style}");
    assert!(style.contains("left:960px"), "{style}");
    assert!(style.contains("top:540px"), "{style}");

    // 宿主收到的最后一条 append_html = 真正插入的元素(带 data-lsm-id)。
    let last = reply
        .outcome
        .actions
        .iter()
        .filter(|a| a.kind == "append_html")
        .last()
        .expect("应有原点层的 append_html");
    assert!(last.value.contains("position:absolute"), "{}", last.value);
    assert_eq!(last.value.matches("<div").count(), 1, "不应有壳包裹:{}", last.value);
}

// ---------- 多顶层节点与 afterbegin 顺序 ----------

#[test]
fn insertAdjacentHTML_多顶层节点保序() {
    let handle = JsHandle::spawn();
    // 全动态容器(快照容器上 afterbegin 有「快照子恒在 dynKids 前」的覆盖层
    // 已知限制,宿主 insert_html 动作本身语义正确,这里测 dynKids 内的顺序)。
    let reply = eval_on(
        &handle,
        r#"(function(){
          var p = document.createElement('div');
          document.body.appendChild(p);
          p.insertAdjacentHTML('beforeend', '<i id="keep">keep</i>');
          p.insertAdjacentHTML('afterbegin', '<div id="A"></div><span id="B">t</span><div id="C"></div>');
          var names = [];
          for (var i = 0; i < p.children.length; i++) names.push(p.children[i].id);
          return JSON.stringify(names);
        })()"#,
        DYN_STAGE_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    let v: serde_json::Value = serde_json::from_str(&reply.outcome.value).unwrap();
    assert_eq!(v, serde_json::json!(["A", "B", "C", "keep"]), "afterbegin 应逆序入位保持文档序:{v}");
}

// ---------- style 驼峰属性名归一化 ----------

#[test]
fn style驼峰属性名归一化为中划线() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"(function(){
          var d = document.createElement('div');
          d.style.zIndex = 5;
          d.style.marginTop = '10px';
          d.style.MozUserSelect = 'none';
          d.style.cssText += ';left:1px';
          document.body.appendChild(d);
          return JSON.stringify({ z: d.style.zIndex, mt: d.style.marginTop, attr: d.getAttribute('style') });
        })()"#,
        DYN_STAGE_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    let v: serde_json::Value = serde_json::from_str(&reply.outcome.value).unwrap();
    let attr = v["attr"].as_str().unwrap();
    assert!(attr.contains("z-index:5"), "style.zIndex 必须归一化为 z-index:{attr}");
    assert!(attr.contains("margin-top:10px"), "{attr}");
    assert!(attr.contains("-moz-user-select:none"), "厂商前缀 Moz* 应转 -moz-*:{attr}");
    assert!(attr.contains("left:1px"), "{attr}");
    assert_eq!(v["z"], "5", "style.zIndex 读回应命中 z-index:{v}");
    assert_eq!(v["mt"], "10px", "{v}");
}

#[test]
fn style驼峰归一化_宿主动作同样生效() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"(function(){
          var d = document.createElement('div');
          d.id = 'layer';
          document.body.appendChild(d);
          d.style.zIndex = 200;
          d.style.left = '0px';
          return d.getAttribute('style');
        })()"#,
        DYN_STAGE_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    // 挂载后的属性写:宿主动作(set_attr)里的 value 也必须是中划线形态。
    let sa = reply
        .outcome
        .actions
        .iter()
        .find(|a| a.kind == "set_attr" && a.key == "style")
        .expect("应有 style set_attr");
    assert!(sa.value.contains("z-index:200"), "宿主动作不应出现驼峰 zIndex:{}", sa.value);
    assert!(!sa.value.contains("zIndex"), "{}", sa.value);
}

// ---------- env 借用断言(与 tests.rs 的 env 保持同一夹具) ----------

#[test]
fn 环境夹具存在性() {
    let _ = env("https://a.test/");
}
