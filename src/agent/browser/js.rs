//! JS 运行时:Boa 引导后端 + DOM 快照桥。
//!
//! 阶段一不执行页面自身脚本,只执行 Agent 的 `eval_js`。每次调用创建独立的
//! Boa Context,把当前 DOM 序列化为快照注入 `document` 兼容层;脚本产生的点击、
//! 跳转、表单值、存储写入通过 `__lsm_actions` 回传给宿主执行。
//!
//! 设计取舍:快照桥保证内核 `Send`(Boa Context / DOM 句柄都不跨 await 保存),
//! 代价是脚本中对 DOM 的结构性修改不会立即反映到下一次布局;阶段二换成 V8 +
//! 原生 DOM 绑定后由同一 trait 接口替换。

use serde::Deserialize;
use serde_json::{json, Value};

use super::document::SnapshotNode;

/// JS 执行结果。
#[derive(Debug, Clone, Default)]
pub struct JsOutcome {
    pub ok: bool,
    pub value: String,
    pub value_type: String,
    pub error: Option<String>,
    pub actions: Vec<JsAction>,
    pub logs: Vec<JsLog>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct JsAction {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub selector: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub event: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct JsLog {
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub text: String,
}

/// 页面元信息(注入 `document`/`window`/`location`/存储)。
pub struct JsPageMeta<'a> {
    pub url: &'a str,
    pub title: &'a str,
    pub cookies: &'a str,
    pub local_storage: &'a std::collections::HashMap<String, String>,
    pub session_storage: &'a std::collections::HashMap<String, String>,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub user_agent: &'a str,
}

const MAX_SCRIPT_BYTES: usize = 256 * 1024;
const MAX_VALUE_CHARS: usize = 256 * 1024;

/// 执行脚本。`nodes` 为当前 DOM 快照。
pub fn eval(script: &str, nodes: &[SnapshotNode], meta: &JsPageMeta<'_>) -> Result<JsOutcome, String> {
    if script.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "脚本超过上限({} KB)",
            MAX_SCRIPT_BYTES / 1024
        ));
    }
    let nodes_json = serde_json::to_string(
        &nodes.iter().map(|n| n.to_json()).collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());
    let meta_json = build_meta(meta);
    let user_json = serde_json::to_string(script).unwrap_or_else(|_| "\"\"".to_string());

    let source = PRELUDE
        .replace("__LSM_NODES__", &nodes_json)
        .replace("__LSM_META__", &meta_json)
        .replace("__LSM_USER__", &user_json);

    let mut ctx = boa_engine::Context::default();
    let value = ctx
        .eval(boa_engine::Source::from_bytes(&source))
        .map_err(|e| format!("JS 执行失败:{e}"))?;
    let text = value
        .to_string(&mut ctx)
        .map_err(|e| format!("JS 结果转换失败:{e}"))?
        .to_std_string_escaped();

    let parsed: Value = serde_json::from_str(&text)
        .map_err(|e| format!("JS 返回结构非法:{e}(原始:{}）", clip(&text, 300)))?;
    let ok = parsed.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let error = parsed
        .get("error")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let value_type = parsed
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("undefined")
        .to_string();
    let raw_value = parsed.get("value").cloned().unwrap_or(Value::Null);
    let value = match raw_value {
        Value::Null => {
            if value_type == "undefined" {
                "undefined".to_string()
            } else {
                "null".to_string()
            }
        }
        Value::String(s) => match serde_json::from_str::<Value>(&s) {
            Ok(Value::String(inner)) => inner,
            Ok(Value::Null) => "null".to_string(),
            Ok(v) => v.to_string(),
            Err(_) => s,
        },
        other => other.to_string(),
    };
    let (value, truncated) = clip_owned(value, MAX_VALUE_CHARS);
    let actions: Vec<JsAction> = parsed
        .get("actions")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let logs: Vec<JsLog> = parsed
        .get("console")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(JsOutcome {
        ok,
        value,
        value_type,
        error,
        actions,
        logs,
        truncated,
    })
}

fn build_meta(meta: &JsPageMeta<'_>) -> String {
    let (protocol, host, hostname, pathname, search, hash, origin) = split_url(meta.url);
    json!({
        "url": meta.url,
        "title": meta.title,
        "cookies": meta.cookies,
        "local": meta.local_storage,
        "session": meta.session_storage,
        "width": meta.viewport_width,
        "height": meta.viewport_height,
        "scrollX": meta.scroll_x,
        "scrollY": meta.scroll_y,
        "ua": meta.user_agent,
        "protocol": protocol,
        "host": host,
        "hostname": hostname,
        "pathname": pathname,
        "search": search,
        "hash": hash,
        "origin": origin,
    })
    .to_string()
}

fn split_url(url: &str) -> (String, String, String, String, String, String, String) {
    match url::Url::parse(url) {
        Ok(u) => (
            u.scheme().to_string(),
            u.host_str().map(|h| match u.port() {
                Some(p) => format!("{h}:{p}"),
                None => h.to_string(),
            }).unwrap_or_default(),
            u.host_str().unwrap_or_default().to_string(),
            u.path().to_string(),
            u.query().map(|q| format!("?{q}")).unwrap_or_default(),
            u.fragment().map(|f| format!("#{f}")).unwrap_or_default(),
            format!("{}://{}", u.scheme(), u.host_str().unwrap_or_default()),
        ),
        Err(_) => (
            "https".into(),
            String::new(),
            String::new(),
            "/".into(),
            String::new(),
            String::new(),
            String::new(),
        ),
    }
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn clip_owned(s: String, n: usize) -> (String, bool) {
    if s.chars().count() <= n {
        return (s, false);
    }
    (s.chars().take(n).collect(), true)
}

/// 注入前导脚本。动态部分用占位符替换。
const PRELUDE: &str = r##"
(function () {
  "use strict";
  var __lsm_nodes = __LSM_NODES__;
  var __lsm_meta = __LSM_META__;
  var __lsm_user = __LSM_USER__;
  var __lsm_actions = [];
  var __lsm_console = [];
  var __lsm_values = {};
  var __lsm_checked = {};
  var __lsm_text = {};
  var __lsm_href = {};
  var __timer_budget = 64;

  function __push(a) { __lsm_actions.push(a); }
  function __matchSimple(n, s) {
    s = String(s || "").trim();
    if (!s) return false;
    var tag = (s.match(/^([a-zA-Z0-9*_-]+)/) || [])[1] || "";
    if (tag && tag !== "*" && tag.toLowerCase() !== String(n.tag || "").toLowerCase()) return false;
    var ids = s.match(/#([A-Za-z0-9_-]+)/g) || [];
    for (var i = 0; i < ids.length; i++) {
      if (String(n.id || "") !== ids[i].slice(1)) return false;
    }
    var classes = String(n.class || "").split(/\s+/);
    var cls = s.match(/\.([A-Za-z0-9_-]+)/g) || [];
    for (var j = 0; j < cls.length; j++) {
      if (classes.indexOf(cls[j].slice(1)) < 0) return false;
    }
    var attrs = s.match(/\[([^\]]+)\]/g) || [];
    for (var k = 0; k < attrs.length; k++) {
      var expr = attrs[k].slice(1, -1);
      var eq = expr.indexOf("=");
      if (eq >= 0) {
        var key = expr.slice(0, eq).trim();
        var val = expr.slice(eq + 1).trim().replace(/^["']|["']$/g, "");
        if (String((n.attrs || {})[key]) !== val) return false;
      } else {
        if (!Object.prototype.hasOwnProperty.call(n.attrs || {}, expr.trim())) return false;
      }
    }
    return true;
  }
  function __match(n, s) {
    s = String(s || "").trim();
    if (!s) return false;
    var parts = s.split(",");
    for (var i = 0; i < parts.length; i++) {
      var sel = parts[i].trim();
      var tokens = sel.split(/\s+/).filter(function (x) { return x && x !== ">"; });
      var right = tokens.length ? tokens[tokens.length - 1] : sel;
      if (__matchSimple(n, right)) return true;
    }
    return false;
  }
  function __query(sel) {
    var out = [];
    for (var i = 0; i < __lsm_nodes.length; i++) {
      if (__match(__lsm_nodes[i], sel)) out.push(__mk(__lsm_nodes[i]));
    }
    return out;
  }
  function __queryOne(sel) {
    var all = __query(sel);
    return all.length ? all[0] : null;
  }
  function __mk(n) {
    var attrs = n.attrs || {};
    var el = {
      __lsm_node: n,
      tagName: String(n.tag || "").toUpperCase(),
      nodeName: String(n.tag || "").toUpperCase(),
      nodeType: 1,
      id: n.id || "",
      className: n.class || "",
      name: n.name || "",
      type: n.type || "",
      textContent: n.text || "",
      innerText: n.text || "",
      innerHTML: n.text || "",
      outerHTML: n.attrs && n.attrs.outer_html ? n.attrs.outer_html : n.text || "",
      checked: !!n.checked,
      disabled: !!attrs.disabled,
      selected: !!attrs.selected,
      style: { getPropertyValue: function () { return ""; } },
      dataset: {},
      classList: {
        contains: function (c) { return String(n.class || "").split(/\s+/).indexOf(c) >= 0; },
        add: function (c) { __push({ type: "set_attr", selector: n.sel, key: "class", value: (n.class ? n.class + " " : "") + c }); },
        remove: function (c) { __push({ type: "set_attr", selector: n.sel, key: "class", value: String(n.class || "").split(/\s+/).filter(function (x) { return x && x !== c; }).join(" ") }); }
      },
      getAttribute: function (k) {
        k = String(k);
        return Object.prototype.hasOwnProperty.call(attrs, k) ? attrs[k] : null;
      },
      hasAttribute: function (k) { return Object.prototype.hasOwnProperty.call(attrs, String(k)); },
      setAttribute: function (k, v) { __push({ type: "set_attr", selector: n.sel, key: String(k), value: String(v) }); attrs[String(k)] = String(v); },
      removeAttribute: function (k) { __push({ type: "remove_attr", selector: n.sel, key: String(k) }); delete attrs[String(k)]; },
      click: function () { __push({ type: "click", selector: n.sel }); },
      focus: function () { __push({ type: "focus", selector: n.sel }); },
      blur: function () { __push({ type: "blur", selector: n.sel }); },
      submit: function () { __push({ type: "submit", selector: n.sel }); },
      dispatchEvent: function (e) { __push({ type: "event", selector: n.sel, event: String((e && e.type) || e) }); return true; },
      addEventListener: function () {},
      removeEventListener: function () {},
      getBoundingClientRect: function () {
        return { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0 };
      },
      scrollIntoView: function () { __push({ type: "scroll_to", selector: n.sel }); },
      scrollTo: function (x, y) { __push({ type: "scroll", value: String(y || 0) }); },
      querySelector: function (s) { var c = el.children; for (var i = 0; i < c.length; i++) { if (__match(c[i].__lsm_node, s)) return c[i]; } return __queryOne(s); },
      querySelectorAll: function (s) { return __query(s); },
      children: [],
      childNodes: [],
      parentNode: null,
      parentElement: null,
      firstChild: null,
      lastChild: null,
      nextSibling: null,
      previousSibling: null,
      value: n.value || "",
      href: n.href || "",
      src: attrs.src || "",
      title: attrs.title || "",
      placeholder: attrs.placeholder || "",
      options: [],
      selectedIndex: -1,
      files: [],
      appendChild: function (c) { return c; },
      removeChild: function (c) { return c; },
      insertBefore: function (c) { return c; },
      remove: function () {},
      cloneNode: function () { return el; }
    };
    function define(name, getter, setter) {
      try { Object.defineProperty(el, name, { get: getter, set: setter, configurable: true, enumerable: true }); } catch (e) {}
    }
    var key = n.sel || ("#idx_" + n.i);
    define("value", function () {
      return Object.prototype.hasOwnProperty.call(__lsm_values, key) ? __lsm_values[key] : (n.value || "");
    }, function (v) {
      __lsm_values[key] = String(v);
      __push({ type: "set_value", selector: n.sel, value: String(v) });
    });
    define("checked", function () {
      return Object.prototype.hasOwnProperty.call(__lsm_checked, key) ? __lsm_checked[key] : !!n.checked;
    }, function (v) {
      __lsm_checked[key] = !!v;
      __push({ type: "set_checked", selector: n.sel, value: v ? "true" : "false" });
    });
    define("textContent", function () {
      return Object.prototype.hasOwnProperty.call(__lsm_text, key) ? __lsm_text[key] : (n.text || "");
    }, function (v) {
      __lsm_text[key] = String(v);
      __push({ type: "set_text", selector: n.sel, value: String(v) });
    });
    define("innerText", function () { return el.textContent; }, function (v) { el.textContent = v; });
    define("innerHTML", function () { return el.textContent; }, function (v) { el.textContent = v; });
    define("href", function () {
      return Object.prototype.hasOwnProperty.call(__lsm_href, key) ? __lsm_href[key] : (n.href || "");
    }, function (v) {
      __lsm_href[key] = String(v);
      __push({ type: "navigate", url: String(v) });
    });
    return el;
  }

  var document = {
    readyState: "complete",
    visibilityState: "visible",
    hidden: false,
    designMode: "off",
    title: __lsm_meta.title,
    URL: __lsm_meta.url,
    documentURI: __lsm_meta.url,
    referrer: "",
    domain: __lsm_meta.hostname,
    location: null,
    characterSet: "UTF-8",
    contentType: "text/html",
    querySelector: __queryOne,
    querySelectorAll: __query,
    getElementById: function (id) { return __queryOne("#" + id); },
    getElementsByTagName: function (t) {
      if (t === "*") return __query("*");
      return __query(String(t));
    },
    getElementsByClassName: function (c) { return __query("." + String(c)); },
    getElementsByName: function (n) { return __query("[name=" + n + "]"); },
    createElement: function (tag) {
      return __mk({ i: -1, sel: "", tag: String(tag || "div"), id: "", class: "", name: "", type: "", value: "", href: "", checked: false, text: "", visible: true, attrs: {} });
    },
    createTextNode: function (t) { return { nodeType: 3, textContent: String(t), data: String(t) }; },
    createDocumentFragment: function () { return { appendChild: function (c) { return c; }, children: [] }; },
    write: function () {},
    writeln: function () {},
    addEventListener: function () {},
    removeEventListener: function () {},
    dispatchEvent: function () { return true; },
    getSelection: function () { return { toString: function () { return ""; }, removeAllRanges: function () {} }; },
    hasFocus: function () { return true; },
    queryCommandSupported: function () { return false; }
  };
  var __doc_nodes = __lsm_nodes.filter(function (n) {
    var t = String(n.tag || "").toLowerCase();
    return t === "html" || t === "head" || t === "body";
  });
  document.documentElement = __doc_nodes.filter(function (n) { return String(n.tag).toLowerCase() === "html"; }).map(__mk)[0] || null;
  document.head = __doc_nodes.filter(function (n) { return String(n.tag).toLowerCase() === "head"; }).map(__mk)[0] || null;
  document.body = __doc_nodes.filter(function (n) { return String(n.tag).toLowerCase() === "body"; }).map(__mk)[0] || null;
  document.forms = __query("form");
  document.links = __query("a[href]");
  document.images = __query("img");
  document.scripts = __query("script");
  try {
    Object.defineProperty(document, "cookie", {
      get: function () { return __lsm_meta.cookies || ""; },
      set: function (v) { __push({ type: "set_cookie", value: String(v) }); },
      configurable: true
    });
    Object.defineProperty(document, "title", {
      get: function () { return __lsm_meta.title || ""; },
      set: function (v) { __lsm_meta.title = String(v); __push({ type: "set_title", value: String(v) }); },
      configurable: true
    });
  } catch (e) {}

  var location = {
    href: __lsm_meta.url,
    protocol: __lsm_meta.protocol,
    host: __lsm_meta.host,
    hostname: __lsm_meta.hostname,
    pathname: __lsm_meta.pathname,
    search: __lsm_meta.search,
    hash: __lsm_meta.hash,
    origin: __lsm_meta.origin,
    assign: function (u) { __push({ type: "navigate", url: String(u) }); },
    replace: function (u) { __push({ type: "navigate", url: String(u) }); },
    reload: function () { __push({ type: "reload" }); },
    toString: function () { return __lsm_meta.url; }
  };
  try {
    Object.defineProperty(location, "href", {
      get: function () { return __lsm_meta.url; },
      set: function (v) { __push({ type: "navigate", url: String(v) }); },
      configurable: true
    });
  } catch (e) {}
  document.location = location;

  function makeStorage(map, kind) {
    var keys = Object.keys(map || {});
    var api = {
      getItem: function (k) { return Object.prototype.hasOwnProperty.call(map, String(k)) ? map[String(k)] : null; },
      setItem: function (k, v) { map[String(k)] = String(v); __push({ type: kind, key: String(k), value: String(v) }); },
      removeItem: function (k) { delete map[String(k)]; __push({ type: kind + "_remove", key: String(k) }); },
      clear: function () { __push({ type: kind + "_clear" }); },
      key: function (i) { return keys[i] || null; }
    };
    try { Object.defineProperty(api, "length", { get: function () { return Object.keys(map).length; } }); } catch (e) {}
    return api;
  }
  var localStorage = makeStorage(__lsm_meta.local || {}, "storage_local");
  var sessionStorage = makeStorage(__lsm_meta.session || {}, "storage_session");

  var navigator = {
    userAgent: __lsm_meta.ua,
    appName: "Netscape",
    appVersion: "5.0 (LsmAgentBrowser)",
    platform: "LsmAgentBrowser",
    language: "zh-CN",
    languages: ["zh-CN", "zh", "en"],
    onLine: true,
    webdriver: false,
    cookieEnabled: true,
    hardwareConcurrency: 4,
    maxTouchPoints: 0
  };
  var screen = {
    width: __lsm_meta.width,
    height: __lsm_meta.height,
    availWidth: __lsm_meta.width,
    availHeight: __lsm_meta.height,
    colorDepth: 24,
    pixelDepth: 24,
    orientation: { type: "landscape-primary", angle: 0 }
  };
  var history = {
    length: 1,
    state: null,
    scrollRestoration: "auto",
    back: function () { __push({ type: "back" }); },
    forward: function () { __push({ type: "forward" }); },
    go: function (n) { __push({ type: n < 0 ? "back" : "forward" }); },
    pushState: function (s, t, u) { if (u) __push({ type: "navigate", url: String(u) }); },
    replaceState: function (s, t, u) { if (u) __push({ type: "navigate", url: String(u) }); }
  };

  function __timer(fn) {
    if (__timer_budget-- > 0 && typeof fn === "function") {
      try { fn(); } catch (e) {}
    }
    return __timer_budget;
  }
  var setTimeout = __timer;
  var setInterval = function (fn) { return __timer(fn); };
  var clearTimeout = function () {};
  var clearInterval = function () {};
  var requestAnimationFrame = function (fn) { return __timer(fn); };
  var cancelAnimationFrame = function () {};

  var console = {
    log: function () { __lsm_console.push({ level: "log", text: Array.prototype.map.call(arguments, String).join(" ") }); },
    info: function () { __lsm_console.push({ level: "info", text: Array.prototype.map.call(arguments, String).join(" ") }); },
    warn: function () { __lsm_console.push({ level: "warn", text: Array.prototype.map.call(arguments, String).join(" ") }); },
    error: function () { __lsm_console.push({ level: "error", text: Array.prototype.map.call(arguments, String).join(" ") }); },
    debug: function () { __lsm_console.push({ level: "debug", text: Array.prototype.map.call(arguments, String).join(" ") }); },
    trace: function () { __lsm_console.push({ level: "trace", text: Array.prototype.map.call(arguments, String).join(" ") }); },
    dir: function () {},
    table: function () {},
    time: function () {},
    timeEnd: function () {}
  };

  var alert = function (m) { __lsm_console.push({ level: "alert", text: String(m || "") }); };
  var confirm = function (m) { __lsm_console.push({ level: "confirm", text: String(m || "") }); return true; };
  var prompt = function (m) { __lsm_console.push({ level: "prompt", text: String(m || "") }); return null; };

  function fetch(url, opts) {
    __push({ type: "fetch", url: String(url) });
    var resp = {
      ok: false, status: 0, statusText: "", url: String(url), redirected: false,
      headers: { get: function () { return null; } },
      text: function () { return ""; },
      json: function () { return {}; },
      arrayBuffer: function () { return new ArrayBuffer(0); },
      clone: function () { return resp; }
    };
    return {
      then: function (f) { try { if (f) f(resp); } catch (e) {} return this; },
      catch: function () { return this; },
      finally: function (f) { try { if (f) f(); } catch (e) {} return this; }
    };
  }
  function XMLHttpRequest() {
    this.readyState = 0; this.status = 0; this.responseText = ""; this.response = "";
    this.onload = null; this.onerror = null; this.onreadystatechange = null;
    this._url = ""; this._headers = {};
  }
  XMLHttpRequest.prototype.open = function (m, u) { this._url = String(u); this.readyState = 1; };
  XMLHttpRequest.prototype.setRequestHeader = function (k, v) { this._headers[String(k)] = String(v); };
  XMLHttpRequest.prototype.send = function () {
    __push({ type: "fetch", url: this._url });
    this.readyState = 4; this.status = 0; this.responseText = "";
    try { if (this.onreadystatechange) this.onreadystatechange(); } catch (e) {}
    try { if (this.onload) this.onload(); } catch (e) {}
  };
  XMLHttpRequest.prototype.addEventListener = function (name, fn) { if (name === "load") this.onload = fn; };
  XMLHttpRequest.prototype.abort = function () {};

  var window = {
    document: document, location: location, navigator: navigator, screen: screen, history: history,
    localStorage: localStorage, sessionStorage: sessionStorage, console: console,
    innerWidth: __lsm_meta.width, innerHeight: __lsm_meta.height,
    outerWidth: __lsm_meta.width, outerHeight: __lsm_meta.height,
    scrollX: __lsm_meta.scrollX || 0, scrollY: __lsm_meta.scrollY || 0,
    pageXOffset: __lsm_meta.scrollX || 0, pageYOffset: __lsm_meta.scrollY || 0,
    devicePixelRatio: 1, name: "", closed: false, isSecureContext: true,
    getComputedStyle: function () { return { getPropertyValue: function () { return ""; } }; },
    matchMedia: function () { return { matches: false, media: "", addListener: function () {}, removeListener: function () {}, addEventListener: function () {}, removeEventListener: function () {} }; },
    addEventListener: function () {}, removeEventListener: function () {}, dispatchEvent: function () { return true; },
    scrollTo: function (x, y) { __push({ type: "scroll", value: String(typeof x === "object" && x ? (x.top || 0) : (y || 0)) }); },
    scrollBy: function (x, y) { __push({ type: "scroll_by", value: String(typeof x === "object" && x ? (x.top || 0) : (y || 0)) }); },
    open: function (u) { __push({ type: "new_tab", url: String(u) }); return null; },
    close: function () {},
    focus: function () {}, blur: function () {},
    setTimeout: setTimeout, setInterval: setInterval, clearTimeout: clearTimeout, clearInterval: clearInterval,
    requestAnimationFrame: requestAnimationFrame, cancelAnimationFrame: cancelAnimationFrame,
    alert: alert, confirm: confirm, prompt: prompt, fetch: fetch, XMLHttpRequest: XMLHttpRequest,
    atob: function (s) { return atob(s); }, btoa: function (s) { return btoa(s); },
    performance: { now: function () { return 0; }, timing: {}, getEntriesByType: function () { return []; } }
  };
  window.self = window; window.top = window; window.parent = window; window.frames = window;
  window.window = window;
  try {
    Object.defineProperty(window, "location", {
      get: function () { return location; },
      set: function (v) { __push({ type: "navigate", url: String(v) }); },
      configurable: true
    });
  } catch (e) {}

  var __params = ["document", "window", "location", "navigator", "screen", "history",
    "localStorage", "sessionStorage", "console", "alert", "confirm", "prompt",
    "fetch", "XMLHttpRequest", "setTimeout", "setInterval", "clearTimeout",
    "clearInterval", "requestAnimationFrame", "cancelAnimationFrame", "performance"];
  var __args = [document, window, location, navigator, screen, history,
    localStorage, sessionStorage, console, alert, confirm, prompt,
    fetch, XMLHttpRequest, setTimeout, setInterval, clearTimeout,
    clearInterval, requestAnimationFrame, cancelAnimationFrame, window.performance];

  var __err = null, __r;
  try {
    try {
      var __fn = new Function(__params.join(","), "return (" + __lsm_user + "\n);");
      __r = __fn.apply(null, __args);
    } catch (e1) {
      if (e1 && e1.name === "SyntaxError") {
        if (/\breturn\b/.test(__lsm_user)) {
          var __fn2 = new Function(__params.join(","), __lsm_user);
          __r = __fn2.apply(null, __args);
        } else {
          __r = eval(__lsm_user);
        }
      } else {
        throw e1;
      }
    }
  } catch (e) {
    __err = String((e && e.stack) || e);
  }
  var __val = null;
  if (__err === null) {
    try { __val = (typeof __r === "undefined") ? null : JSON.stringify(__r); }
    catch (e2) { __val = String(__r); }
  }
  return JSON.stringify({
    ok: __err === null,
    value: __val,
    type: typeof __r,
    error: __err,
    actions: __lsm_actions,
    console: __lsm_console
  });
})()
"##;

#[cfg(test)]
mod tests {
    use super::*;

    fn node(tag: &str, id: &str, class: &str, text: &str, href: &str) -> SnapshotNode {
        SnapshotNode {
            index: 0,
            parent: usize::MAX,
            selector: if id.is_empty() {
                tag.to_string()
            } else {
                format!("#{id}")
            },
            tag: tag.to_string(),
            id: id.to_string(),
            class: class.to_string(),
            name: String::new(),
            typ: String::new(),
            value: String::new(),
            href: href.to_string(),
            checked: false,
            text: text.to_string(),
            visible: true,
            attrs: Default::default(),
            css: Default::default(),
        }
    }

    fn meta<'a>(local: &'a std::collections::HashMap<String, String>) -> JsPageMeta<'a> {
        JsPageMeta {
            url: "https://a.test/p?q=1",
            title: "T",
            cookies: "a=1",
            local_storage: local,
            session_storage: local,
            viewport_width: 800,
            viewport_height: 600,
            scroll_x: 0.0,
            scroll_y: 0.0,
            user_agent: "LsmAgentBrowser",
        }
    }

    #[test]
    fn 表达式与文档属性() {
        let local = Default::default();
        let nodes = vec![node("h1", "", "", "Hello", "")];
        let out = eval("document.title + '|' + document.querySelector('h1').innerText", &nodes, &meta(&local)).unwrap();
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(out.value, "T|Hello");
    }

    #[test]
    fn 点击产生动作() {
        let local = Default::default();
        let nodes = vec![node("a", "go", "btn", "Go", "https://a.test/x")];
        let out = eval("document.getElementById('go').click(); document.location.href='https://a.test/y'; 1", &nodes, &meta(&local)).unwrap();
        assert!(out.ok, "{:?}", out.error);
        assert!(out.actions.iter().any(|a| a.kind == "click" && a.selector == "#go"));
        assert!(out.actions.iter().any(|a| a.kind == "navigate" && a.url == "https://a.test/y"));
    }

    #[test]
    fn 输入值写入动作() {
        let local = Default::default();
        let nodes = vec![node("input", "q", "", "", "")];
        let out = eval("document.querySelector('#q').value='rust'; document.querySelector('#q').value", &nodes, &meta(&local)).unwrap();
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(out.value, "rust");
        assert!(out.actions.iter().any(|a| a.kind == "set_value" && a.value == "rust"));
    }

    #[test]
    fn 存储与_cookie_动作() {
        let local = Default::default();
        let nodes = vec![];
        let out = eval("localStorage.setItem('k','v'); document.cookie='x=1'; localStorage.getItem('k')", &nodes, &meta(&local)).unwrap();
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(out.value, "v");
        assert!(out.actions.iter().any(|a| a.kind == "storage_local"));
        assert!(out.actions.iter().any(|a| a.kind == "set_cookie"));
    }

    #[test]
    fn 语法错误返回结构化失败() {
        let local = Default::default();
        let out = eval("this is not js", &[], &meta(&local)).unwrap();
        assert!(!out.ok);
        assert!(out.error.is_some());
    }

    #[test]
    fn 语句块与_return_均可执行() {
        let local = Default::default();
        let out = eval("var a = 2; return a * 3;", &[], &meta(&local)).unwrap();
        assert!(out.ok, "{:?}", out.error);
        assert_eq!(out.value, "6");
    }
}
