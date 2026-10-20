//! PRELUDE v2:JS 线程内的页面环境(持久全局)。
//!
//! 由 Rust 在 Context 创建后一次性求值;之后每次请求经 `__lsmRefresh` 注入最新
//! DOM 快照与页面元信息。与阶段一快照桥(v1)的差异见
//! `docs/自定义浏览器和V8引擎/05-阶段二页面脚本与动态DOM设计.md` 第四节。

/// 原生函数由 Rust 注册(`__lsmFetchNative`/`atob`/`btoa`),此处只做 JS 侧装配。
pub const PRELUDE: &str = r##"
var __lsmNodes = [];
var __lsmMeta = {};
var __lsmActions = [];
var __lsmConsole = [];
var __lsmValues = {};
var __lsmChecked = {};
var __lsmText = {};
var __lsmHref = {};
var __lsmLocal = {};
var __lsmSession = {};
var __lsmCookie = "";
var __lsmWriteBuf = [];
var __lsmListeners = [];
var __lsmTimers = [];
var __lsmTimerSeq = 0;
var __lsmNow = 0;
var __lsmTimerRan = 0;
var __lsmFatal = null;
var __lsmDispatchResult = { handled: false, defaultPrevented: false };

var MAX_ACTIONS = 4096;
var MAX_CONSOLE = 2048;

function __push(a) { if (__lsmActions.length < MAX_ACTIONS) __lsmActions.push(a); }
function __clog(level, text) {
  if (__lsmConsole.length < MAX_CONSOLE) __lsmConsole.push({ level: level, text: String(text) });
}
function __cerror(text) { __clog("error", text); }

function __lsmRefresh(nodes, meta) {
  __lsmNodes = nodes || [];
  __lsmMeta = meta || {};
  __lsmLocal = {};
  var src = __lsmMeta.local || {};
  for (var k0 in src) if (Object.prototype.hasOwnProperty.call(src, k0)) __lsmLocal[k0] = String(src[k0]);
  __lsmSession = {};
  var ss = __lsmMeta.session || {};
  for (var k1 in ss) if (Object.prototype.hasOwnProperty.call(ss, k1)) __lsmSession[k1] = String(ss[k1]);
  __lsmCookie = String(__lsmMeta.cookies || "");
  navigator.userAgent = String(__lsmMeta.ua || "");
  __lsmSetViewport(Number(__lsmMeta.width) || 1280, Number(__lsmMeta.height) || 720);
  window.scrollX = Number(__lsmMeta.scrollX) || 0;
  window.scrollY = Number(__lsmMeta.scrollY) || 0;
  window.pageXOffset = window.scrollX;
  window.pageYOffset = window.scrollY;
  __rebindDocNodes();
  __rebindLocation();
  document.readyState = "loading";
}

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
  for (var i = 0; i < __lsmNodes.length; i++) {
    if (__match(__lsmNodes[i], sel)) out.push(__mk(__lsmNodes[i]));
  }
  return out;
}
function __queryOne(sel) {
  var all = __query(sel);
  return all.length ? all[0] : null;
}
function __findBySel(sel) {
  for (var i = 0; i < __lsmNodes.length; i++) {
    if (__lsmNodes[i].sel === sel || __match(__lsmNodes[i], sel)) return __lsmNodes[i];
  }
  return null;
}
function __ancestorChain(idx) {
  var chain = [];
  var cur = idx;
  var guard = 0;
  while (cur != null && cur >= 0 && cur < __lsmNodes.length && guard++ < 64) {
    chain.push(cur);
    var p = __lsmNodes[cur].parent;
    if (p == null || p < 0 || p >= __lsmNodes.length) break;
    cur = p;
  }
  return chain;
}

// ---------- HTML 轻量序列化(detached 元素树 → HTML 字符串) ----------
function __escHtml(s) {
  return String(s == null ? "" : s)
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
function __escAttr(s) {
  return __escHtml(s).replace(/"/g, "&quot;").replace(/\n/g, " ");
}
var __VOID_TAGS = { br: 1, img: 1, input: 1, meta: 1, link: 1, hr: 1, source: 1, area: 1, embed: 1, col: 1 };
function __serEl(el) {
  if (!el) return "";
  if (el.nodeType === 3) return __escHtml(el.textContent || "");
  var tag = String(el.__tag || el.tagName || "div").toLowerCase();
  var attrs = el.__attrs || {};
  var out = "<" + tag;
  for (var k in attrs) {
    if (!Object.prototype.hasOwnProperty.call(attrs, k)) continue;
    out += " " + k + '="' + __escAttr(attrs[k]) + '"';
  }
  out += ">";
  if (__VOID_TAGS[tag]) return out;
  var kids = el.__kids || [];
  for (var i = 0; i < kids.length; i++) {
    var c = kids[i];
    out += (c.nodeType === 3) ? __escHtml(c.textContent || "") : __serEl(c);
  }
  if (el.__text) out += __escHtml(el.__text);
  out += "</" + tag + ">";
  return out;
}

// ---------- CSS style Proxy ----------
function __cssGet(css, prop) {
  var m = String(css || "").match(new RegExp("(?:^|;)\\s*" + String(prop).replace(/[-]/g, "\\-") + "\\s*:\\s*([^;]+)", "i"));
  return m ? m[1].trim() : "";
}
function __cssSet(css, prop, val) {
  css = String(css || "");
  var re = new RegExp("(^|;)\\s*" + String(prop).replace(/[-]/g, "\\-") + "\\s*:[^;]*(;|$)", "i");
  var pair = prop + ":" + val;
  if (re.test(css)) return css.replace(re, function (m) {
    var lead = m.charAt(0) === ";" ? ";" : "";
    var tail = m.charAt(m.length - 1) === ";" ? ";" : "";
    return (lead ? lead : "") + pair + tail;
  });
  return (css ? css.replace(/;\s*$/, "") + ";" : "") + pair + ";";
}
function __mkStyle(getCss, setCss) {
  if (typeof Proxy !== "function") {
    return { cssText: getCss(), getPropertyValue: function (p) { return __cssGet(getCss(), p); } };
  }
  var store = {};
  return new Proxy(store, {
    get: function (t, k) {
      if (k === "cssText") return getCss();
      if (k === "getPropertyValue") return function (p) { return __cssGet(getCss(), p); };
      if (k === "setProperty") {
        return function (p, v) { setCss(__cssSet(getCss(), String(p), String(v))); };
      }
      return __cssGet(getCss(), String(k));
    },
    set: function (t, k, v) {
      setCss(__cssSet(getCss(), String(k), String(v)));
      return true;
    }
  });
}

// ---------- 元素工厂 ----------
function __mk(n) {
  var attrs = n.attrs || {};
  var el = {
    __lsm_node: n,
    __lsm_idx: n.i,
    tagName: String(n.tag || "").toUpperCase(),
    nodeName: String(n.tag || "").toUpperCase(),
    nodeType: 1,
    id: n.id || "",
    className: n.class || "",
    name: n.name || "",
    type: n.type || "",
    textContent: n.text || "",
    innerText: n.text || "",
    checked: !!n.checked,
    disabled: !!attrs.disabled,
    selected: !!attrs.selected,
    dataset: {},
    children: [],
    childNodes: [],
    parentNode: null,
    parentElement: null,
    firstChild: null,
    lastChild: null,
    nextSibling: null,
    previousSibling: null,
    options: [],
    selectedIndex: -1,
    files: [],
    getAttribute: function (k) {
      k = String(k);
      return Object.prototype.hasOwnProperty.call(attrs, k) ? attrs[k] : null;
    },
    hasAttribute: function (k) { return Object.prototype.hasOwnProperty.call(attrs, String(k)); },
    setAttribute: function (k, v) {
      __push({ type: "set_attr", selector: n.sel, key: String(k), value: String(v) });
      attrs[String(k)] = String(v);
    },
    removeAttribute: function (k) {
      __push({ type: "remove_attr", selector: n.sel, key: String(k) });
      delete attrs[String(k)];
    },
    click: function () {
      var res = __fire(n.sel, "click");
      __push({ type: "click", selector: n.sel });
      return undefined;
    },
    focus: function () { __push({ type: "focus", selector: n.sel }); },
    blur: function () { __push({ type: "blur", selector: n.sel }); },
    submit: function () { __push({ type: "submit", selector: n.sel }); },
    dispatchEvent: function (e) {
      var t = String((e && e.type) || e);
      __fire(n.sel, t);
      return true;
    },
    addEventListener: function (type, fn) {
      if (typeof fn === "function") __lsmListeners.push({ sel: n.sel, type: String(type), fn: fn });
    },
    removeEventListener: function () {},
    getBoundingClientRect: function () {
      return { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0 };
    },
    scrollIntoView: function () { __push({ type: "scroll_to", selector: n.sel }); },
    scrollTo: function (x, y) { __push({ type: "scroll", value: String(y || 0) }); },
    querySelector: function (s) { return __queryOne(s); },
    querySelectorAll: function (s) { return __query(s); },
    appendChild: function (c) {
      if (c && c.__detached) __push({ type: "append_html", selector: n.sel, value: __serEl(c) });
      return c;
    },
    insertBefore: function (c) {
      if (c && c.__detached) __push({ type: "insert_html", selector: n.sel, value: __serEl(c) });
      return c;
    },
    removeChild: function (c) {
      if (c && c.__lsm_node) __push({ type: "remove_node", selector: c.__lsm_node.sel });
      return c;
    },
    remove: function () { __push({ type: "remove_node", selector: n.sel }); },
    cloneNode: function () { return el; }
  };
  el.classList = {
    contains: function (c) { return String(n.class || "").split(/\s+/).indexOf(c) >= 0; },
    add: function (c) {
      var cls = String(el.className || "").split(/\s+/).filter(function (x) { return x && x !== c; });
      cls.push(String(c));
      el.className = cls.join(" ");
      __push({ type: "set_attr", selector: n.sel, key: "class", value: el.className });
    },
    remove: function (c) {
      var cls = String(el.className || "").split(/\s+/).filter(function (x) { return x && x !== c; });
      el.className = cls.join(" ");
      __push({ type: "set_attr", selector: n.sel, key: "class", value: el.className });
    }
  };
  function define(name, getter, setter) {
    try { Object.defineProperty(el, name, { get: getter, set: setter, configurable: true, enumerable: true }); } catch (e) {}
  }
  var key = n.sel || ("#idx_" + n.i);
  define("value", function () {
    return Object.prototype.hasOwnProperty.call(__lsmValues, key) ? __lsmValues[key] : (n.value || "");
  }, function (v) {
    __lsmValues[key] = String(v);
    __push({ type: "set_value", selector: n.sel, value: String(v) });
  });
  define("checked", function () {
    return Object.prototype.hasOwnProperty.call(__lsmChecked, key) ? __lsmChecked[key] : !!n.checked;
  }, function (v) {
    __lsmChecked[key] = !!v;
    __push({ type: "set_checked", selector: n.sel, value: v ? "true" : "false" });
  });
  define("textContent", function () {
    return Object.prototype.hasOwnProperty.call(__lsmText, key) ? __lsmText[key] : (n.text || "");
  }, function (v) {
    __lsmText[key] = String(v);
    __push({ type: "set_text", selector: n.sel, value: String(v) });
  });
  define("innerText", function () { return el.textContent; }, function (v) { el.textContent = v; });
  define("innerHTML", function () {
    return Object.prototype.hasOwnProperty.call(__lsmText, key) ? __lsmText[key] : (n.attrs && n.attrs.outer_html ? n.attrs.outer_html : n.text || "");
  }, function (v) {
    var s = String(v);
    __lsmText[key] = s;
    __push({ type: "set_html", selector: n.sel, value: s });
  });
  define("outerHTML", function () {
    return (n.attrs && n.attrs.outer_html) || n.text || "";
  }, function (v) {
    __push({ type: "replace_html", selector: n.sel, value: String(v) });
  });
  define("href", function () {
    return Object.prototype.hasOwnProperty.call(__lsmHref, key) ? __lsmHref[key] : (n.href || "");
  }, function (v) {
    __lsmHref[key] = String(v);
    __push({ type: "navigate", url: String(v) });
  });
  define("src", function () { return attrs.src || ""; }, function (v) {
    __push({ type: "set_attr", selector: n.sel, key: "src", value: String(v) });
    attrs.src = String(v);
  });
  var styleCss = attrs.style || "";
  el.style = __mkStyle(function () { return styleCss; }, function (css) {
    styleCss = css;
    __push({ type: "set_attr", selector: n.sel, key: "style", value: css });
    attrs.style = css;
  });
  return el;
}

function __newEl(tag) {
  var t = String(tag || "div").toLowerCase();
  var el = {
    __detached: true,
    __tag: t,
    __attrs: {},
    __kids: [],
    __text: "",
    nodeType: 1,
    tagName: t.toUpperCase(),
    nodeName: t.toUpperCase(),
    className: "",
    id: "",
    style: __mkStyle(function () { return el.__attrs.style || ""; }, function (css) { el.__attrs.style = css; }),
    dataset: {},
    children: [],
    childNodes: [],
    setAttribute: function (k, v) { this.__attrs[String(k)] = String(v); if (k === "id") this.id = String(v); if (k === "class") this.className = String(v); },
    getAttribute: function (k) { return Object.prototype.hasOwnProperty.call(this.__attrs, String(k)) ? this.__attrs[String(k)] : null; },
    hasAttribute: function (k) { return Object.prototype.hasOwnProperty.call(this.__attrs, String(k)); },
    removeAttribute: function (k) { delete this.__attrs[String(k)]; },
    appendChild: function (c) { this.__kids.push(c); return c; },
    insertBefore: function (c) { this.__kids.push(c); return c; },
    removeChild: function (c) {
      var i = this.__kids.indexOf(c);
      if (i >= 0) this.__kids.splice(i, 1);
      return c;
    },
    remove: function () {},
    addEventListener: function () {},
    removeEventListener: function () {},
    click: function () {},
    focus: function () {},
    blur: function () {},
    cloneNode: function () { return __newEl(t); },
    querySelector: function () { return null; },
    querySelectorAll: function () { return []; },
    getBoundingClientRect: function () { return { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0 }; }
  };
  function def(name, getter, setter) {
    try { Object.defineProperty(el, name, { get: getter, set: setter, configurable: true, enumerable: true }); } catch (e) {}
  }
  def("innerHTML", function () { return __serEl({ __tag: t, __attrs: {}, __kids: el.__kids, __text: "" }); },
    function (v) {
      el.__kids = [];
      el.__text = "";
      __push({ type: "set_html", selector: "@created:" + t + ":" + el.__attrs.id, value: String(v) });
    });
  def("textContent", function () { return el.__text; }, function (v) { el.__text = String(v); el.__kids = []; });
  def("innerText", function () { return el.__text; }, function (v) { el.__text = String(v); el.__kids = []; });
  def("value", function () { return el.__attrs.value || ""; }, function (v) { el.__attrs.value = String(v); });
  return el;
}

// ---------- document ----------
var document = {
  readyState: "loading",
  visibilityState: "visible",
  hidden: false,
  designMode: "off",
  title: "",
  URL: "",
  documentURI: "",
  referrer: "",
  domain: "",
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
  createElement: function (tag) { return __newEl(tag); },
  createTextNode: function (t) { return { nodeType: 3, textContent: String(t), data: String(t) }; },
  createDocumentFragment: function () {
    return { __detached: true, __tag: "div", __attrs: {}, __kids: [], __text: "", nodeType: 11, appendChild: function (c) { this.__kids.push(c); return c; } };
  },
  write: function () {
    for (var i = 0; i < arguments.length; i++) __lsmWriteBuf.push(String(arguments[i]));
  },
  writeln: function () {
    for (var i = 0; i < arguments.length; i++) __lsmWriteBuf.push(String(arguments[i]) + "\n");
  },
  addEventListener: function (type, fn) {
    if (typeof fn === "function") __lsmListeners.push({ sel: "@document", type: String(type), fn: fn });
  },
  removeEventListener: function () {},
  dispatchEvent: function (e) { __fireDocument(String((e && e.type) || e)); return true; },
  getSelection: function () { return { toString: function () { return ""; }, removeAllRanges: function () {} }; },
  hasFocus: function () { return true; },
  queryCommandSupported: function () { return false; }
};
var __doc_nodes = [];
function __rebindDocNodes() {
  __doc_nodes = __lsmNodes.filter(function (n) {
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
}
try {
  Object.defineProperty(document, "cookie", {
    get: function () { return __lsmCookie; },
    set: function (v) {
      var line = String(v);
      __lsmCookieApply([line]);
      __push({ type: "set_cookie", value: line });
    },
    configurable: true
  });
  Object.defineProperty(document, "title", {
    get: function () { return String(__lsmMeta.title || ""); },
    set: function (v) {
      __lsmMeta.title = String(v);
      __push({ type: "set_title", value: String(v) });
    },
    configurable: true
  });
} catch (e) {}

function __lsmCookieApply(lines) {
  var map = {};
  String(__lsmCookie || "").split(/;\s*/).forEach(function (kv) {
    var i = kv.indexOf("=");
    if (i > 0) map[kv.slice(0, i)] = kv.slice(i + 1);
  });
  (lines || []).forEach(function (line) {
    var pair = String(line).split(";")[0];
    var i = pair.indexOf("=");
    if (i <= 0) return;
    var name = pair.slice(0, i).trim();
    var val = pair.slice(i + 1).trim();
    if (/expires=Thu, 01 Jan 1970/i.test(String(line))) delete map[name];
    else map[name] = val;
  });
  __lsmCookie = Object.keys(map).map(function (k) { return k + "=" + map[k]; }).join("; ");
}

// ---------- location ----------
var location = {
  assign: function (u) { __push({ type: "navigate", url: String(u) }); },
  replace: function (u) { __push({ type: "navigate", url: String(u) }); },
  reload: function () { __push({ type: "reload" }); },
  toString: function () { return String(__lsmMeta.url || ""); }
};
function __rebindLocation() {
  location.protocol = String(__lsmMeta.protocol || "https:");
  location.host = String(__lsmMeta.host || "");
  location.hostname = String(__lsmMeta.hostname || "");
  location.pathname = String(__lsmMeta.pathname || "/");
  location.search = String(__lsmMeta.search || "");
  location.hash = String(__lsmMeta.hash || "");
  location.origin = String(__lsmMeta.origin || "");
  location.href = String(__lsmMeta.url || "");
}
document.location = location;

// ---------- storage ----------
function makeStorage(map, kind) {
  var api = {
    getItem: function (k) { return Object.prototype.hasOwnProperty.call(map, String(k)) ? map[String(k)] : null; },
    setItem: function (k, v) { map[String(k)] = String(v); __push({ type: kind, key: String(k), value: String(v) }); },
    removeItem: function (k) { delete map[String(k)]; __push({ type: kind + "_remove", key: String(k) }); },
    clear: function () {
      var keys = Object.keys(map);
      for (var i = 0; i < keys.length; i++) delete map[keys[i]];
      __push({ type: kind + "_clear" });
    },
    key: function (i) { return Object.keys(map)[i] || null; }
  };
  try { Object.defineProperty(api, "length", { get: function () { return Object.keys(map).length; } }); } catch (e) {}
  return api;
}
var localStorage = makeStorage(__lsmLocal, "storage_local");
var sessionStorage = makeStorage(__lsmSession, "storage_session");

var navigator = {
  userAgent: "",
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
  width: 1280, height: 720, availWidth: 1280, availHeight: 720,
  colorDepth: 24, pixelDepth: 24,
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

// ---------- 定时器(虚拟时钟) ----------
function __lsmDelay(ms) {
  var d = Number(ms);
  if (!isFinite(d) || d < 0) d = 0;
  if (d > 60000) d = 60000;
  return d;
}
function setTimeout(fn, delay) {
  var id = ++__lsmTimerSeq;
  if (typeof fn === "function") {
    var args = Array.prototype.slice.call(arguments, 2);
    __lsmTimers.push({ id: id, fn: fn, args: args, next: __lsmNow + __lsmDelay(delay), interval: __lsmDelay(delay), repeat: false });
  }
  return id;
}
function setInterval(fn, delay) {
  var id = ++__lsmTimerSeq;
  if (typeof fn === "function") {
    var args = Array.prototype.slice.call(arguments, 2);
    var d = Math.max(16, __lsmDelay(delay));
    __lsmTimers.push({ id: id, fn: fn, args: args, next: __lsmNow + d, interval: d, repeat: true });
  }
  return id;
}
function clearTimeout(id) {
  __lsmTimers = __lsmTimers.filter(function (t) { return t.id !== id; });
}
function clearInterval(id) { clearTimeout(id); }
function requestAnimationFrame(fn) { return setTimeout(fn, 16); }
function cancelAnimationFrame(id) { clearTimeout(id); }
function __lsmTick() {
  var ran = 0;
  while (ran < 64) {
    var best = -1;
    for (var i = 0; i < __lsmTimers.length; i++) {
      if (best < 0 || __lsmTimers[i].next < __lsmTimers[best].next) best = i;
    }
    if (best < 0) break;
    var due = __lsmTimers[best].next;
    __lsmNow = Math.max(__lsmNow, due);
    var batch = [], rest = [];
    for (var j = 0; j < __lsmTimers.length; j++) {
      if (__lsmTimers[j].next <= due) batch.push(__lsmTimers[j]);
      else rest.push(__lsmTimers[j]);
    }
    __lsmTimers = rest;
    for (var k = 0; k < batch.length; k++) {
      var tm = batch[k];
      if (tm.repeat && __lsmTimerRan < 4096) {
        __lsmTimers.push({ id: tm.id, fn: tm.fn, args: tm.args, next: due + (tm.interval || 16), interval: tm.interval || 16, repeat: true });
      }
      ran++;
      __lsmTimerRan++;
      try { tm.fn.apply(null, tm.args || []); } catch (e) { __cerror("timer: " + String((e && e.stack) || e)); }
    }
    if (ran >= 64) break;
  }
  return JSON.stringify({ pending: __lsmTimers.length, ran: ran });
}

// ---------- console ----------
var console = {
  log: function () { __clog("log", Array.prototype.map.call(arguments, function (x) { return __str(x); }).join(" ")); },
  info: function () { __clog("info", Array.prototype.map.call(arguments, function (x) { return __str(x); }).join(" ")); },
  warn: function () { __clog("warn", Array.prototype.map.call(arguments, function (x) { return __str(x); }).join(" ")); },
  error: function () { __clog("error", Array.prototype.map.call(arguments, function (x) { return __str(x); }).join(" ")); },
  debug: function () { __clog("debug", Array.prototype.map.call(arguments, function (x) { return __str(x); }).join(" ")); },
  trace: function () { __clog("trace", Array.prototype.map.call(arguments, function (x) { return __str(x); }).join(" ")); },
  dir: function () {},
  table: function () {},
  time: function () {},
  timeEnd: function () {}
};
function __str(x) {
  if (x === null) return "null";
  if (x === undefined) return "undefined";
  if (typeof x === "object") {
    try { return JSON.stringify(x); } catch (e) { return String(x); }
  }
  return String(x);
}

var alert = function (m) { __clog("alert", String(m == null ? "" : m)); };
var confirm = function (m) { __clog("confirm", String(m == null ? "" : m)); return true; };
var prompt = function (m) { __clog("prompt", String(m == null ? "" : m)); return null; };

// ---------- fetch / XHR ----------
function fetch(url, opts) {
  return Promise.resolve().then(function () {
    var urlString = String(url && url.url ? url.url : url);
    var o = { method: (opts && opts.method) || "GET", cookies: __lsmCookie };
    if (opts && opts.headers) o.headers = opts.headers;
    if (opts && opts.body != null) o.body = typeof opts.body === "string" ? opts.body : String(opts.body);
    var raw = "";
    try { raw = __lsmFetchNative(urlString, JSON.stringify(o)); } catch (e2) { raw = JSON.stringify({ error: String(e2) }); }
    var r = {};
    try { r = JSON.parse(raw); } catch (e3) { r = { error: "原生 fetch 返回非法结构" }; }
    if (r.setCookie && r.setCookie.length) __lsmCookieApply(r.setCookie);
    var resp = {
      ok: !!r.ok,
      status: r.status || 0,
      statusText: String(r.statusText || ""),
      url: String(r.url || urlString),
      redirected: !!r.redirect,
      headers: { get: function (k) { return (r.headers && r.headers[String(k).toLowerCase()]) || null; } },
      bodyUsed: false,
      clone: function () { return resp; },
      _read: function () {
        if (r.error) throw new TypeError(r.error);
        this.bodyUsed = true;
        return r.body == null ? "" : String(r.body);
      },
      text: function () { var s = this; return Promise.resolve().then(function () { return s._read(); }); },
      json: function () { var s = this; return Promise.resolve().then(function () { return JSON.parse(s._read()); }); }
    };
    return resp;
  });
}
function XMLHttpRequest() {
  this.readyState = 0; this.status = 0; this.statusText = ""; this.responseText = ""; this.response = "";
  this.onload = null; this.onerror = null; this.onreadystatechange = null;
  this._url = ""; this._method = "GET"; this._headers = {}; this._listeners = {};
}
XMLHttpRequest.prototype.open = function (m, u) {
  this._method = String(m || "GET").toUpperCase();
  this._url = String(u);
  this.readyState = 1;
  this.__rs();
};
XMLHttpRequest.prototype.setRequestHeader = function (k, v) { this._headers[String(k)] = String(v); };
XMLHttpRequest.prototype.addEventListener = function (name, fn) {
  if (!this._listeners[name]) this._listeners[name] = [];
  this._listeners[name].push(fn);
};
XMLHttpRequest.prototype.__fire = function (name) {
  var list = this._listeners[name] || [];
  for (var i = 0; i < list.length; i++) { try { list[i].call(this); } catch (e) { __cerror("xhr " + name + ": " + String(e)); } }
};
XMLHttpRequest.prototype.__rs = function () {
  if (this.onreadystatechange) { try { this.onreadystatechange(); } catch (e) { __cerror("xhr rs: " + String(e)); } }
};
XMLHttpRequest.prototype.send = function (body) {
  var o = { method: this._method, headers: this._headers, cookies: __lsmCookie };
  if (body != null) o.body = typeof body === "string" ? body : String(body);
  var raw = "";
  try { raw = __lsmFetchNative(this._url, JSON.stringify(o)); } catch (e2) { raw = JSON.stringify({ error: String(e2) }); }
  var r = {};
  try { r = JSON.parse(raw); } catch (e3) { r = { error: "原生 fetch 返回非法结构" }; }
  if (r.setCookie && r.setCookie.length) __lsmCookieApply(r.setCookie);
  if (r.error) {
    this.readyState = 4; this.status = 0;
    this.__rs();
    if (this.onerror) { try { this.onerror(); } catch (e4) {} }
    this.__fire("error");
    return;
  }
  this.readyState = 2; this.__rs();
  this.readyState = 3; this.__rs();
  this.readyState = 4;
  this.status = r.status || 0;
  this.statusText = String(r.statusText || "");
  this.responseText = r.body == null ? "" : String(r.body);
  this.response = this.responseText;
  this.__rs();
  if (this.onload) { try { this.onload(); } catch (e5) { __cerror("xhr load: " + String(e5)); } }
  this.__fire("load");
};
XMLHttpRequest.prototype.abort = function () {};
XMLHttpRequest.prototype.getResponseHeader = function (k) {
  return (this._respHeaders && this._respHeaders[String(k).toLowerCase()]) || null;
};

// ---------- window ----------
var window = {
  document: document, location: location, navigator: navigator, screen: screen, history: history,
  localStorage: localStorage, sessionStorage: sessionStorage, console: console,
  innerWidth: 1280, innerHeight: 720, outerWidth: 1280, outerHeight: 720,
  scrollX: 0, scrollY: 0, pageXOffset: 0, pageYOffset: 0,
  devicePixelRatio: 1, name: "", closed: false, isSecureContext: true,
  getComputedStyle: function (el) {
    var css = "";
    try { css = String((el && el.style && el.style.cssText) || ""); } catch (e) {}
    return { getPropertyValue: function (p) { return __cssGet(css, p); } };
  },
  matchMedia: function () {
    return { matches: false, media: "", addListener: function () {}, removeListener: function () {}, addEventListener: function () {}, removeEventListener: function () {} };
  },
  addEventListener: function (type, fn) {
    if (typeof fn === "function") __lsmListeners.push({ sel: "@window", type: String(type), fn: fn });
  },
  removeEventListener: function () {},
  dispatchEvent: function (e) { __fireDocument(String((e && e.type) || e)); return true; },
  scrollTo: function (x, y) { __push({ type: "scroll", value: String(typeof x === "object" && x ? (x.top || 0) : (y || 0)) }); },
  scrollBy: function (x, y) { __push({ type: "scroll_by", value: String(typeof x === "object" && x ? (x.top || 0) : (y || 0)) }); },
  open: function (u) { __push({ type: "new_tab", url: String(u) }); return null; },
  close: function () {},
  focus: function () {}, blur: function () {},
  setTimeout: setTimeout, setInterval: setInterval, clearTimeout: clearTimeout, clearInterval: clearInterval,
  requestAnimationFrame: requestAnimationFrame, cancelAnimationFrame: cancelAnimationFrame,
  alert: alert, confirm: confirm, prompt: prompt, fetch: fetch, XMLHttpRequest: XMLHttpRequest,
  atob: atob, btoa: btoa,
  performance: {
    now: function () { return __lsmNow; },
    timing: {},
    getEntriesByType: function () { return []; }
  }
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

// ---------- 事件系统 ----------
function __mkEvent(type) {
  return {
    type: String(type), target: null, currentTarget: null, timeStamp: __lsmNow,
    defaultPrevented: false, propagationStopped: false,
    preventDefault: function () { this.defaultPrevented = true; },
    stopPropagation: function () { this.propagationStopped = true; },
    stopImmediatePropagation: function () { this.propagationStopped = true; }
  };
}
function __fire(sel, type) {
  var node = __findBySel(sel);
  if (!node) return { handled: false, defaultPrevented: false };
  var chain = __ancestorChain(node.i);
  var handled = false, prevented = false;
  for (var i = 0; i < __lsmListeners.length; i++) {
    var L = __lsmListeners[i];
    if (L.type !== String(type)) continue;
    var targetIdx = -1;
    if (L.sel === "@document" || L.sel === "@window") {
      targetIdx = -2;
    } else {
      for (var j = 0; j < chain.length; j++) {
        var n2 = __lsmNodes[chain[j]];
        if (n2 && __match(n2, L.sel)) { targetIdx = chain[j]; break; }
      }
    }
    if (targetIdx === -1) continue;
    var ev = __mkEvent(type);
    ev.target = __mk(node);
    ev.currentTarget = targetIdx === -2 ? document : __mk(__lsmNodes[targetIdx]);
    handled = true;
    try { L.fn(ev); } catch (e) { __cerror("listener " + type + ": " + String((e && e.stack) || e)); }
    if (ev.defaultPrevented) prevented = true;
    if (ev.propagationStopped) break;
  }
  return { handled: handled, defaultPrevented: prevented };
}
function __fireDocument(type) {
  var handled = false, prevented = false;
  for (var i = 0; i < __lsmListeners.length; i++) {
    var L = __lsmListeners[i];
    if (L.type !== String(type)) continue;
    if (L.sel !== "@document" && L.sel !== "@window") continue;
    var ev = __mkEvent(type);
    ev.target = document;
    ev.currentTarget = L.sel === "@window" ? window : document;
    handled = true;
    try { L.fn(ev); } catch (e) { __cerror("doc listener " + type + ": " + String((e && e.stack) || e)); }
    if (ev.defaultPrevented) prevented = true;
    if (ev.propagationStopped) break;
  }
  return { handled: handled, defaultPrevented: prevented };
}

// ---------- 宿主驱动入口 ----------
function __lsmRunScript(src) {
  try { (0, eval)(String(src)); } catch (e) { __cerror(String((e && e.stack) || e)); }
  return true;
}
function __lsmEvalUser(src) {
  var __err = null, __r;
  try {
    try { __r = (0, eval)("(" + src + "\n)"); }
    catch (e1) {
      if (e1 && e1.name === "SyntaxError") {
        if (/\breturn\b/.test(src)) { __r = (new Function(src))(); }
        else { __r = (0, eval)(src); }
      } else { throw e1; }
    }
  } catch (e) { __err = String((e && e.stack) || e); }
  var __val = null;
  if (__err === null) {
    try { __val = (typeof __r === "undefined") ? null : JSON.stringify(__r); }
    catch (e2) { __val = String(__r); }
  }
  return JSON.stringify({ ok: __err === null, value: __val, type: typeof __r, error: __err });
}
function __lsmDispatch(sel, type) {
  var res = __fire(String(sel), String(type));
  __lsmDispatchResult = res;
  return JSON.stringify(res);
}
function __lsmDispatchDoc(type) {
  var res = __fireDocument(String(type));
  __lsmDispatchResult = res;
  return JSON.stringify(res);
}
function __lsmReady(state) { document.readyState = String(state); }
function __lsmBegin() {
  __lsmActions.length = 0;
  __lsmConsole.length = 0;
  __lsmFatal = null;
  __lsmDispatchResult = { handled: false, defaultPrevented: false };
}
function __lsmFlushWrites() {
  for (var i = 0; i < __lsmWriteBuf.length; i++) {
    __push({ type: "append_html", selector: "body", value: __lsmWriteBuf[i] });
  }
  __lsmWriteBuf = [];
}
function __lsmSetUa(ua) { navigator.userAgent = String(ua || ""); }
function __lsmSetViewport(w, h) {
  window.innerWidth = w; window.innerHeight = h;
  window.outerWidth = w; window.outerHeight = h;
  screen.width = w; screen.height = h;
  screen.availWidth = w; screen.availHeight = h;
}
function __lsmCollect(ov) {
  ov = ov || {};
  return JSON.stringify({
    ok: ("ok" in ov) ? !!ov.ok : __lsmFatal === null,
    value: ("value" in ov) ? ov.value : null,
    type: ("type" in ov) ? ov.type : "undefined",
    error: ("error" in ov) ? ov.error : __lsmFatal,
    actions: __lsmActions,
    console: __lsmConsole,
    handled: __lsmDispatchResult.handled,
    defaultPrevented: __lsmDispatchResult.defaultPrevented
  });
}
__lsmFatal = null;
"##;
