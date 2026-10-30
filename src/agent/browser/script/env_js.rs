//! 快照选择器引擎(阶段一/阶段二 PRELUDE 共用)。
//!
//! 快照桥的选择器只能"要么命中、要么不命中":遇到内核不支持的语法必须明确
//! 不匹配,**绝不能静默退化成"匹配全部"** —— 那会让 Agent 拿到一堆无关
//! 节点却以为命中了目标(实测 `div:hover` 曾静默返回页面内全部 `div`)。
//!
//! 与宿主侧动作落点(`dom_ops::find` → kuchikiki 选择器)保持语义一致:
//! 同一套选择器在 `inspect(elements)` 与 `eval_js` 里必须指向同一个元素。
//!
//! 依赖的宿主侧符号:
//! - `__lsmNodes`:快照节点数组(阶段一/阶段二统一用此名);
//! - `__mk(snapshotNode)`:节点 → JS 元素对象的工厂。
//!
//! 支持子集:标签 / `*` / `#id` / `.class` / `[attr]` / `[attr=v]` /
//! `[attr^=v]` / `[attr$=v]` / `[attr*=v]` / `[attr~=v]` / `[attr|=v]` /
//! 伪类 `:first-child` `:last-child` `:only-child` `:nth-child()` `:nth-of-type()`
//! `:nth-last-child()` `:not()` `:is()` `:where()` `:empty` `:root` `:link`
//! `:checked` `:disabled` `:enabled` `:required` / 后代组合器 / 子代组合器(`>`) /
//! 分组(`,`)。
//!
//! 状态类伪类(`:hover` `:focus` `:active` `:visited` 等)内核无交互状态,
//! 一律**不匹配**;未知伪类同样不匹配。

pub const SELECTOR_JS: &str = r##"
// ---------- 快照选择器引擎(阶段一/阶段二共用) ----------
function __lsmParent(idx) {
  if (idx == null || idx < 0 || idx >= __lsmNodes.length) return -1;
  var p = __lsmNodes[idx].parent;
  if (p == null || p < 0 || p >= __lsmNodes.length) return -1;
  return p;
}
function __lsmSiblings(idx) {
  var out = [];
  var p = __lsmParent(idx);
  if (p < 0) {
    if (idx >= 0 && idx < __lsmNodes.length) out.push(idx);
    return out;
  }
  for (var i = 0; i < __lsmNodes.length; i++) {
    if (__lsmNodes[i].parent === p) out.push(i);
  }
  return out;
}
function __lsmAttrMatch(n, expr) {
  var attrs = n.attrs || {};
  var m = String(expr).match(/^\s*([A-Za-z0-9_:.-]+)\s*(?:([~^$*|]?=)\s*([\s\S]*?))?\s*$/);
  if (!m) return false;
  var key = m[1];
  var op = m[2];
  if (!op) return Object.prototype.hasOwnProperty.call(attrs, key);
  if (!Object.prototype.hasOwnProperty.call(attrs, key)) return false;
  var val = String(m[3] == null ? "" : m[3]).replace(/^["']|["']$/g, "");
  var actual = String(attrs[key]);
  if (op === "=") return actual === val;
  if (op === "^=") return val !== "" && actual.indexOf(val) === 0;
  if (op === "$=") return val !== "" && actual.length >= val.length && actual.lastIndexOf(val) === actual.length - val.length;
  if (op === "*=") return val !== "" && actual.indexOf(val) >= 0;
  if (op === "~=") return actual.split(/\s+/).indexOf(val) >= 0;
  if (op === "|=") return actual === val || actual.indexOf(val + "-") === 0;
  return false;
}
function __lsmNth(arg, pos) {
  var a = String(arg == null ? "" : arg).trim().toLowerCase();
  if (a === "odd") return pos % 2 === 1;
  if (a === "even") return pos % 2 === 0;
  var m = a.match(/^([+-]?\d*)n\s*(?:([+-])\s*(\d+))?$/);
  if (m) {
    var step = m[1] === "" || m[1] === "+" ? 1 : (m[1] === "-" ? -1 : parseInt(m[1], 10));
    var offset = m[3] ? (m[2] === "-" ? -parseInt(m[3], 10) : parseInt(m[3], 10)) : 0;
    if (step === 0) return pos === offset;
    var k = (pos - offset) / step;
    return k >= 0 && Math.floor(k) === k;
  }
  var num = parseInt(a, 10);
  return !isNaN(num) && pos === num;
}
function __lsmPseudo(idx, name, arg) {
  var n = __lsmNodes[idx];
  if (!n) return false;
  var sibs = __lsmSiblings(idx);
  var pos = sibs.indexOf(idx) + 1;
  var tag = String(n.tag || "").toLowerCase();
  var typePos = 0;
  for (var i = 0; i < sibs.length; i++) {
    if (String(__lsmNodes[sibs[i]].tag || "").toLowerCase() === tag) {
      typePos++;
      if (sibs[i] === idx) break;
    }
  }
  if (name === "first-child") return pos === 1;
  if (name === "last-child") return pos === sibs.length;
  if (name === "only-child") return sibs.length === 1;
  if (name === "nth-child") return __lsmNth(arg, pos);
  if (name === "nth-last-child") return __lsmNth(arg, sibs.length - pos + 1);
  if (name === "nth-of-type") return __lsmNth(arg, typePos);
  if (name === "not") return !__lsmMatchSel(idx, arg);
  if (name === "is" || name === "where" || name === "matches") return __lsmMatchSel(idx, arg);
  if (name === "empty") return !(n.text || "").trim();
  if (name === "root") return __lsmParent(idx) < 0;
  if (name === "link") return !!n.href;
  if (name === "checked") return !!n.checked;
  if (name === "disabled") return !!(n.attrs && n.attrs.disabled);
  if (name === "enabled") return !(n.attrs && n.attrs.disabled);
  if (name === "required") return !!(n.attrs && n.attrs.required);
  // 第 41 轮:交互态伪类按运行时状态属性命中(内核私有 `data-lsm-*` 标记,
  // 由 `Document::apply_widget_state` 写入临时解析树)。
  var attrs = n.attrs || {};
  function __lsmHas(a) { return Object.prototype.hasOwnProperty.call(attrs, a); }
  if (name === "hover") return __lsmHas("data-lsm-hover");
  if (name === "focus") return __lsmHas("data-lsm-focus");
  if (name === "focus-visible") return __lsmHas("data-lsm-focus-visible");
  if (name === "active") return __lsmHas("data-lsm-active");
  if (name === "indeterminate") return __lsmHas("indeterminate");
  // 其余状态类伪类:内核无对应状态 → 不匹配(绝不匹配全部)。
  if (name === "focus-within" || name === "visited" || name === "target" ||
      name === "read-only" || name === "read-write") return false;
  // 未知伪类:不匹配。
  return false;
}
function __lsmMatchSimple(idx, compound) {
  var n = __lsmNodes[idx];
  if (!n) return false;
  var s = String(compound == null ? "" : compound).trim();
  if (!s) return false;
  var attrRe = /\[([^\]]*)\]/g;
  var am;
  while ((am = attrRe.exec(s)) !== null) {
    if (!__lsmAttrMatch(n, am[1])) return false;
  }
  var rest = s.replace(attrRe, "");
  var pseudos = [];
  var pre = rest.replace(/:([A-Za-z][A-Za-z0-9_-]*)(?:\(([^()]*)\))?/g, function (_m, pname, parg) {
    pseudos.push({ name: String(pname).toLowerCase(), arg: parg == null ? "" : String(parg) });
    return "";
  });
  var tag = (pre.match(/^([a-zA-Z0-9*_-]+)/) || [])[1] || "";
  if (tag && tag !== "*" && tag.toLowerCase() !== String(n.tag || "").toLowerCase()) return false;
  var ids = pre.match(/#([A-Za-z0-9_-]+)/g) || [];
  for (var i = 0; i < ids.length; i++) {
    if (String(n.id || "") !== ids[i].slice(1)) return false;
  }
  var classes = String(n.class || "").split(/\s+/);
  var cls = pre.match(/\.([A-Za-z0-9_-]+)/g) || [];
  for (var j = 0; j < cls.length; j++) {
    if (classes.indexOf(cls[j].slice(1)) < 0) return false;
  }
  for (var k = 0; k < pseudos.length; k++) {
    if (!__lsmPseudo(idx, pseudos[k].name, pseudos[k].arg)) return false;
  }
  return true;
}
function __lsmParseSel(sel) {
  var raw = String(sel).replace(/>/g, " > ").split(/\s+/).filter(function (x) { return x; });
  var out = [];
  var pending = " ";
  for (var i = 0; i < raw.length; i++) {
    if (raw[i] === ">") { pending = ">"; continue; }
    out.push({ sel: raw[i], comb: pending });
    pending = " ";
  }
  return out;
}
function __lsmMatchChain(idx, compounds) {
  if (!compounds.length) return false;
  var ci = compounds.length - 1;
  if (!__lsmMatchSimple(idx, compounds[ci].sel)) return false;
  var cur = idx;
  for (var k = ci - 1; k >= 0; k--) {
    var comb = compounds[k + 1].comb;
    if (comb === ">") {
      var p = __lsmParent(cur);
      if (p < 0 || !__lsmMatchSimple(p, compounds[k].sel)) return false;
      cur = p;
    } else {
      var found = -1;
      var q = __lsmParent(cur);
      var guard = 0;
      while (q >= 0 && guard++ < 64) {
        if (__lsmMatchSimple(q, compounds[k].sel)) { found = q; break; }
        q = __lsmParent(q);
      }
      if (found < 0) return false;
      cur = found;
    }
  }
  return true;
}
function __lsmMatchSel(idx, sel) {
  var s = String(sel == null ? "" : sel).trim();
  if (!s) return false;
  var groups = s.split(",");
  for (var g = 0; g < groups.length; g++) {
    var one = groups[g].trim();
    if (!one) continue;
    if (__lsmMatchChain(idx, __lsmParseSel(one))) return true;
  }
  return false;
}
function __lsmIsDescendant(idx, rootIdx) {
  var p = __lsmParent(idx);
  var guard = 0;
  while (p >= 0 && guard++ < 64) {
    if (p === rootIdx) return true;
    p = __lsmParent(p);
  }
  return false;
}
// rootIdx >= 0 时只在 rootIdx 的**后代**内查找(元素作用域 querySelector,
// 与真实 DOM 一致:元素自身的 querySelectorAll 不会命中自己)。
function __lsmQuery(sel, rootIdx) {
  var out = [];
  var scoped = (rootIdx != null && rootIdx >= 0);
  for (var i = 0; i < __lsmNodes.length; i++) {
    if (scoped && !__lsmIsDescendant(i, rootIdx)) continue;
    if (__lsmMatchSel(i, sel)) out.push(__mk(__lsmNodes[i]));
  }
  return out;
}
function __lsmQueryOne(sel, rootIdx) {
  var all = __lsmQuery(sel, rootIdx);
  return all.length ? all[0] : null;
}
function __lsmFindBySel(sel) {
  for (var i = 0; i < __lsmNodes.length; i++) {
    if (__lsmNodes[i].sel === sel || __lsmMatchSel(i, sel)) return __lsmNodes[i];
  }
  return null;
}
"##;

/// 常见标准全局(阶段一/阶段二共用)。
///
/// 站点脚本在初始化阶段就会碰这些全局;缺失一个就抛 `ReferenceError`,
/// 页面主内容永远建不出来(实测百度地图的 `URL is not defined` / `alog is not defined`)。
/// 这里按"够用即可"的原则补齐最小实现,行为与浏览器保持一致的部分优先。
pub const GLOBALS_JS: &str = r##"
// ---------- 常见标准全局(够用即可) ----------
(function () {
  if (typeof globalThis === "undefined") return;
  var G = globalThis;

  // URLSearchParams 的安全百分号解码:裸 decodeURIComponent 遇孤立 %
  // 会抛 URIError,浏览器语义按原样保留。此前 URLSearchParams 引用了
  // 从未定义的 __lsmDecode,任何 `new URL(...)` 都当场 ReferenceError
  // (实测 zhipin app~3 的路由解析)。
  function __lsmDecode(s) {
    try { return decodeURIComponent(String(s).replace(/\+/g, " ")); } catch (e) { return String(s); }
  }

  // --- URL / URLSearchParams ---
  if (typeof G.URL === "undefined") {
    var URLImpl = function URL(u, base) {
      var s = String(u == null ? "" : u);
      var pm = /^[a-zA-Z][a-zA-Z0-9+.-]*:/.exec(s);
      if (!pm && base != null) {
        var b = String(base);
        var bm = /^([a-zA-Z][a-zA-Z0-9+.-]*:)\/\/([^\/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/.exec(b);
        if (bm) {
          var origin = bm[1] + "//" + bm[2];
          var bpath = bm[3] || "/";
          if (s.charAt(0) === "/") s = origin + s;
          else if (s.charAt(0) === "?") s = origin + bpath + s;
          else if (s.charAt(0) === "#") s = origin + bpath + (bm[4] || "") + s;
          else s = origin + bpath.replace(/[^/]*$/, "") + s;
        }
      }
      var pm2 = /^[a-zA-Z][a-zA-Z0-9+.-]*:/.exec(s);
      var scheme = pm2 ? pm2[0].toLowerCase() : "";
      var rest = pm2 ? s.slice(pm2[0].length) : s;
      var hm = /^([^/?#]*)/.exec(rest);
      var host = hm ? hm[1] : "";
      var rp = rest.slice(host.length);
      var hi = rp.indexOf("#");
      var hash = hi >= 0 ? rp.slice(hi) : "";
      if (hi >= 0) rp = rp.slice(0, hi);
      var qi = rp.indexOf("?");
      var query = qi >= 0 ? rp.slice(qi) : "";
      if (qi >= 0) rp = rp.slice(0, qi);
      var pathname = rp || "/";
      var port = "", hostOnly = host;
      var pm3 = /^(.*):(\d+)$/.exec(host);
      if (pm3) { hostOnly = pm3[1]; port = pm3[2]; }
      this.href = s;
      this.protocol = scheme;
      this.host = host;
      this.hostname = hostOnly;
      this.port = port;
      this.origin = (scheme && hostOnly) ? scheme + "//" + hostOnly : "";
      this.pathname = pathname;
      this.search = query;
      this.hash = hash;
      this.searchParams = new G.URLSearchParams(query.replace(/^\?/, ""));
    };
    URLImpl.prototype = { constructor: URLImpl, toString: function () { return this.href; } };
    G.URL = URLImpl;
  }
  if (typeof G.URLSearchParams === "undefined") {
    G.URLSearchParams = function URLSearchParams(init) {
      var pairs = [];
      var s = String(init == null ? "" : init).replace(/^\?/, "");
      if (s) {
        var parts = s.split("&");
        for (var i = 0; i < parts.length; i++) {
          if (!parts[i]) continue;
          var eq = parts[i].indexOf("=");
          var k = eq >= 0 ? parts[i].slice(0, eq) : parts[i];
          var v = eq >= 0 ? parts[i].slice(eq + 1) : "";
          pairs.push([__lsmDecode(k), __lsmDecode(v)]);
        }
      }
      this.get = function (k) {
        k = String(k);
        for (var i = 0; i < pairs.length; i++) if (pairs[i][0] === k) return pairs[i][1];
        return null;
      };
      this.getAll = function (k) {
        k = String(k);
        var out = [];
        for (var i = 0; i < pairs.length; i++) if (pairs[i][0] === k) out.push(pairs[i][1]);
        return out;
      };
      this.has = function (k) { return this.get(k) !== null; };
      this.append = function (k, v) { pairs.push([String(k), String(v)]); };
      this.set = function (k, v) {
        k = String(k);
        var seen = false;
        for (var i = 0; i < pairs.length; i++) {
          if (pairs[i][0] === k) { if (!seen) { pairs[i][1] = String(v); seen = true; } else pairs.splice(i--, 1); }
        }
        if (!seen) pairs.push([k, String(v)]);
      };
      this.delete = function (k) {
        k = String(k);
        for (var i = 0; i < pairs.length; i++) if (pairs[i][0] === k) { pairs.splice(i, 1); i--; }
      };
      this.forEach = function (fn) {
        for (var i = 0; i < pairs.length; i++) fn(pairs[i][1], pairs[i][0]);
      };
      this.toString = function () {
        var out = [];
        for (var i = 0; i < pairs.length; i++) {
          out.push(__lsmEnc(pairs[i][0]) + "=" + __lsmEnc(pairs[i][1]));
        }
        return out.join("&");
      };
    };
  }
  function __lsmEnc(s) {
    return String(s).replace(/%/g, "%25").replace(/&/g, "%26").replace(/\+/g, "%2B").replace(/=/g, "%3D");
  }
  function __lsmDec(s) {
    s = String(s).replace(/\+/g, " ");
    try { return decodeURIComponent(s); } catch (e) { return s; }
  }
  G.__lsmEnc = __lsmEnc;
  G.__lsmDec = __lsmDec;

  // --- Event / CustomEvent ---
  if (typeof G.Event === "undefined") {
    G.Event = function Event(type, init) {
      init = init || {};
      this.type = String(type);
      this.bubbles = !!init.bubbles;
      this.cancelable = !!init.cancelable;
      this.defaultPrevented = false;
      this.target = null;
      this.currentTarget = null;
      this.timeStamp = 0;
      this.preventDefault = function () { this.defaultPrevented = true; };
      this.stopPropagation = function () {};
      this.stopImmediatePropagation = function () {};
    };
  }
  if (typeof G.CustomEvent === "undefined") {
    G.CustomEvent = function CustomEvent(type, init) {
      init = init || {};
      G.Event.call(this, type, init);
      this.detail = init.detail === undefined ? null : init.detail;
    };
    G.CustomEvent.prototype = Object.create(G.Event.prototype);
  }
  // Event 原型上的标准方法(实例自有方法仅供内部构造的旧对象;站点新建的
  // 子类实例走原型链)。
  (function () {
    var P = G.Event.prototype;
    if (!P.preventDefault) P.preventDefault = function () { this.defaultPrevented = true; };
    if (!P.stopPropagation) P.stopPropagation = function () {};
    if (!P.stopImmediatePropagation) P.stopImmediatePropagation = function () {};
    if (!P.composedPath) P.composedPath = function () {
      return this.target ? [this.target, G.document, G.window] : [];
    };
    if (!P.initEvent) P.initEvent = function (type, bubbles, cancelable) {
      this.type = String(type);
      this.bubbles = !!bubbles;
      this.cancelable = !!cancelable;
    };
  })();
  // --- 事件子类构造器 ---
  // 站点与反爬脚本用 `new MouseEvent/KeyboardEvent(...)` 造事件(实测百度地图
  // 反爬脚本直接 `new MouseEvent('mousedown', ...)`;缺失即 ReferenceError,
  // 后续风控回调整层不执行)。以通用工厂按字段清单补齐,行为差异只在字段。
  (function () {
    function evtCtor(name, baseName, fields, defaults) {
      if (typeof G[name] !== "undefined") return;
      var Base = G[baseName] || G.Event;
      var C = function (type, init) {
        init = init || {};
        G.Event.call(this, String(type), init);
        this.view = init.view || G.window || null;
        this.detail = init.detail === undefined ? 0 : init.detail;
        if (fields) {
          for (var i = 0; i < fields.length; i++) {
            var f = fields[i];
            if (init[f] !== undefined) this[f] = init[f];
            else if (defaults && defaults[f] !== undefined) this[f] = defaults[f];
            else this[f] = null;
          }
        }
        if (init.ctrlKey !== undefined) this.ctrlKey = !!init.ctrlKey;
        if (init.shiftKey !== undefined) this.shiftKey = !!init.shiftKey;
        if (init.altKey !== undefined) this.altKey = !!init.altKey;
        if (init.metaKey !== undefined) this.metaKey = !!init.metaKey;
        if (this.ctrlKey === undefined) this.ctrlKey = false;
        if (this.shiftKey === undefined) this.shiftKey = false;
        if (this.altKey === undefined) this.altKey = false;
        if (this.metaKey === undefined) this.metaKey = false;
        var mods = { Control: !!this.ctrlKey, Shift: !!this.shiftKey, Alt: !!this.altKey, Meta: !!this.metaKey };
        this.getModifierState = function (k) { return !!mods[String(k)]; };
      };
      C.prototype = Object.create(Base.prototype);
      C.prototype.constructor = C;
      G[name] = C;
    }
    var ui = ["detail", "view"];
    var mouse = ["screenX", "screenY", "clientX", "clientY", "pageX", "pageY",
                 "offsetX", "offsetY", "movementX", "movementY", "button", "buttons",
                 "relatedTarget", "region"];
    var mouseDefaults = { screenX: 0, screenY: 0, clientX: 0, clientY: 0, pageX: 0, pageY: 0,
                          offsetX: 0, offsetY: 0, movementX: 0, movementY: 0, button: 0, buttons: 0 };
    evtCtor("UIEvent", "Event", ui, { detail: 0 });
    evtCtor("MouseEvent", "UIEvent", mouse, mouseDefaults);
    evtCtor("PointerEvent", "MouseEvent",
            ["pointerId", "pointerType", "isPrimary", "pressure", "tangentialPressure",
             "tiltX", "tiltY", "twist", "width", "height"],
            { pointerId: 0, pointerType: "mouse", isPrimary: true, pressure: 0,
              tangentialPressure: 0, tiltX: 0, tiltY: 0, twist: 0, width: 1, height: 1 });
    evtCtor("WheelEvent", "MouseEvent", ["deltaX", "deltaY", "deltaZ", "deltaMode"],
            { deltaX: 0, deltaY: 0, deltaZ: 0, deltaMode: 0 });
    evtCtor("KeyboardEvent", "UIEvent",
            ["key", "code", "keyCode", "which", "charCode", "location", "repeat",
             "isComposing", "locale"],
            { key: "", code: "", keyCode: 0, which: 0, charCode: 0, location: 0,
              repeat: false, isComposing: false });
    // keyCode/which 的常用键位回填(老库仍按数值判断)。
    (function () {
      var C = G.KeyboardEvent;
      if (!C || C.__lsmKeyCodes) return;
      C.__lsmKeyCodes = true;
      var table = { Enter: 13, Escape: 27, Tab: 9, Backspace: 8, Delete: 46, " ": 32,
                    ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40,
                    Home: 36, End: 35, PageUp: 33, PageDown: 34,
                    Shift: 16, Control: 17, Alt: 18, Meta: 91 };
      var orig = C;
      var Wrapped = function (type, init) {
        init = init || {};
        orig.call(this, type, init);
        if (!this.keyCode && init.keyCode === undefined && table[this.key] !== undefined) {
          this.keyCode = table[this.key];
          this.which = table[this.key];
        }
      };
      Wrapped.prototype = orig.prototype;
      G.KeyboardEvent = Wrapped;
    })();
    evtCtor("FocusEvent", "UIEvent", ["relatedTarget"]);
    evtCtor("InputEvent", "UIEvent", ["data", "inputType", "isComposing", "dataTransfer"],
            { isComposing: false });
    evtCtor("CompositionEvent", "UIEvent", ["data", "locale"]);
    evtCtor("DragEvent", "MouseEvent", ["dataTransfer"]);
    evtCtor("ClipboardEvent", "Event", ["clipboardData"]);
    evtCtor("MessageEvent", "Event", ["data", "origin", "lastEventId", "source", "ports"],
            { origin: "", lastEventId: "" });
    evtCtor("ErrorEvent", "Event", ["message", "filename", "lineno", "colno", "error"],
            { message: "", filename: "", lineno: 0, colno: 0 });
    evtCtor("ProgressEvent", "Event", ["lengthComputable", "loaded", "total"],
            { lengthComputable: false, loaded: 0, total: 0 });
    evtCtor("AnimationEvent", "Event", ["animationName", "elapsedTime", "pseudoElement"],
            { animationName: "", elapsedTime: 0, pseudoElement: "" });
    evtCtor("TransitionEvent", "Event", ["propertyName", "elapsedTime", "pseudoElement"],
            { propertyName: "", elapsedTime: 0, pseudoElement: "" });
    evtCtor("PopStateEvent", "Event", ["state"]);
    evtCtor("HashChangeEvent", "Event", ["oldURL", "newURL"], { oldURL: "", newURL: "" });
    evtCtor("StorageEvent", "Event", ["key", "oldValue", "newValue", "url", "storageArea"],
            { key: "", url: "" });
    evtCtor("BeforeUnloadEvent", "Event", ["returnValue"]);
    evtCtor("PageTransitionEvent", "Event", ["persisted"], { persisted: false });
    evtCtor("SubmitEvent", "Event", ["submitter"]);
    evtCtor("TouchEvent", "UIEvent", ["touches", "targetTouches", "changedTouches", "altKey"],
            { touches: [], targetTouches: [], changedTouches: [] });
    if (typeof G.Touch === "undefined") {
      G.Touch = function Touch(init) {
        init = init || {};
        this.identifier = init.identifier || 0;
        this.target = init.target || null;
        this.clientX = init.clientX || 0;
        this.clientY = init.clientY || 0;
        this.pageX = init.pageX || this.clientX;
        this.pageY = init.pageY || this.clientY;
        this.screenX = init.screenX || 0;
        this.screenY = init.screenY || 0;
        this.radiusX = init.radiusX || 0;
        this.radiusY = init.radiusY || 0;
        this.rotationAngle = init.rotationAngle || 0;
        this.force = init.force || 0;
      };
    }
  })();
  if (typeof G.DOMException === "undefined") {
    G.DOMException = function DOMException(message, name) {
      this.message = String(message == null ? "" : message);
      this.name = String(name || "Error");
      this.code = 0;
      this.stack = this.name + ": " + this.message;
    };
    G.DOMException.prototype = Object.create(typeof G.Error === "function" ? G.Error.prototype : Object.prototype);
  }

  // --- DOM 接口占位(脚本常用 instanceof / new) ---
  if (typeof G.Node === "undefined") {
    G.Node = function Node() {};
    G.Node.ELEMENT_NODE = 1;
    G.Node.TEXT_NODE = 3;
    G.Node.COMMENT_NODE = 8;
    G.Node.DOCUMENT_NODE = 9;
  }
  if (typeof G.Element === "undefined") {
    G.Element = function Element() {};
    G.Element.prototype = Object.create(G.Node.prototype);
  }
  if (typeof G.HTMLElement === "undefined") {
    G.HTMLElement = function HTMLElement() {};
    G.HTMLElement.prototype = Object.create(G.Element.prototype);
  }
  if (typeof G.SVGElement === "undefined") {
    G.SVGElement = function SVGElement() {};
    G.SVGElement.prototype = Object.create(G.Element.prototype);
  }
  // --- Document / HTMLDocument 构造器 ---
  // 反爬与指纹脚本普遍直接读 `window.Document.prototype.…`(实测百度地图
  // abclite 脚本:`gb['Document']['prototype']['createElement']['m']`)。
  // 缺失时属性链在 undefined 上继续取成员 → TypeError,整段反爬脚本中断,
  // BMAP_SECKEY 不落地,依赖它的地图内核不启动(页面只剩外壳)。
  if (typeof G.Document === "undefined") {
    G.Document = function Document() {};
    G.Document.prototype = Object.create(G.Node.prototype);
    G.Document.prototype.constructor = G.Document;
    G.Document.prototype.createElement = function (tag) {
      return (G.document && G.document.createElement) ? G.document.createElement(tag) : null;
    };
    G.Document.prototype.createElementNS = function (ns, tag) {
      return (G.document && G.document.createElementNS) ? G.document.createElementNS(ns, tag) : null;
    };
    G.Document.prototype.createTextNode = function (t) {
      return G.document ? G.document.createTextNode(t) : null;
    };
    G.Document.prototype.createComment = function (t) {
      return G.document ? G.document.createComment(t) : null;
    };
    G.Document.prototype.createDocumentFragment = function () {
      return G.document ? G.document.createDocumentFragment() : null;
    };
    G.Document.prototype.getElementById = function (id) {
      return G.document ? G.document.getElementById(id) : null;
    };
    G.Document.prototype.querySelector = function (s) {
      return G.document ? G.document.querySelector(s) : null;
    };
    G.Document.prototype.querySelectorAll = function (s) {
      return G.document ? G.document.querySelectorAll(s) : [];
    };
  }
  if (typeof G.HTMLDocument === "undefined") {
    G.HTMLDocument = function HTMLDocument() {};
    G.HTMLDocument.prototype = Object.create(G.Document.prototype);
    G.HTMLDocument.prototype.constructor = G.HTMLDocument;
  }
  if (typeof G.DocumentFragment === "undefined") {
    G.DocumentFragment = function DocumentFragment() {};
    G.DocumentFragment.prototype = Object.create(G.Node.prototype);
  }
  if (typeof G.CharacterData === "undefined") {
    G.CharacterData = function CharacterData() {};
    G.CharacterData.prototype = Object.create(G.Node.prototype);
  }
  if (typeof G.Text === "undefined") {
    G.Text = function Text(data) { this.data = String(data == null ? "" : data); };
    G.Text.prototype = Object.create(G.CharacterData.prototype);
  }
  if (typeof G.Comment === "undefined") {
    G.Comment = function Comment(data) { this.data = String(data == null ? "" : data); };
    G.Comment.prototype = Object.create(G.CharacterData.prototype);
  }
  if (typeof G.Attr === "undefined") {
    G.Attr = function Attr() {};
    G.Attr.prototype = Object.create(G.Node.prototype);
  }
  // 各 HTML*Element 构造器:站点/框架做 instanceof 与原型补丁时引用。
  (function () {
    var names = [
      "HTMLHtmlElement", "HTMLHeadElement", "HTMLBodyElement", "HTMLDivElement",
      "HTMLSpanElement", "HTMLAnchorElement", "HTMLInputElement", "HTMLButtonElement",
      "HTMLFormElement", "HTMLImageElement", "HTMLCanvasElement", "HTMLScriptElement",
      "HTMLLinkElement", "HTMLStyleElement", "HTMLSelectElement", "HTMLOptionElement",
      "HTMLTextAreaElement", "HTMLTableElement", "HTMLTableRowElement",
      "HTMLTableCellElement", "HTMLLIElement", "HTMLUListElement", "HTMLOListElement",
      "HTMLParagraphElement", "HTMLMetaElement", "HTMLLabelElement",
      "HTMLTemplateElement", "HTMLIFrameElement", "HTMLVideoElement",
      "HTMLAudioElement", "HTMLMediaElement", "HTMLEmbedElement",
      "HTMLObjectElement", "HTMLAreaElement", "HTMLMapElement", "HTMLBaseElement",
      "HTMLTitleElement", "HTMLBRElement", "HTMLHRElement", "HTMLPreElement",
      "HTMLQuoteElement", "HTMLModElement", "HTMLDListElement", "HTMLFieldSetElement",
      "HTMLLegendElement", "HTMLOptGroupElement", "HTMLOutputElement",
      "HTMLProgressElement", "HTMLMeterElement", "HTMLDetailsElement",
      "HTMLDialogElement", "HTMLSlotElement"
    ];
    for (var i = 0; i < names.length; i++) {
      if (typeof G[names[i]] !== "undefined") continue;
      var C = function () {};
      C.prototype = Object.create(G.HTMLElement.prototype);
      C.prototype.constructor = C;
      G[names[i]] = C;
    }
  })();
  // 文档实例的真实构造器(站点用 `document.constructor === HTMLDocument` 做环境自证)。
  if (G.document && G.HTMLDocument) {
    try { G.document.constructor = G.HTMLDocument; } catch (e) {}
  }
  // NodeFilter:站点 polyfill(createTreeWalker/createNodeIterator)普遍引用
  // 全局 NodeFilter 常量,缺失即 ReferenceError(实测百度地图探针)。
  if (typeof G.NodeFilter === "undefined") {
    G.NodeFilter = {
      SHOW_ALL: 0xFFFFFFFF,
      SHOW_ELEMENT: 0x1,
      SHOW_ATTRIBUTE: 0x2,
      SHOW_TEXT: 0x4,
      SHOW_CDATA_SECTION: 0x8,
      SHOW_ENTITY_REFERENCE: 0x10,
      SHOW_ENTITY: 0x20,
      SHOW_PROCESSING_INSTRUCTION: 0x40,
      SHOW_COMMENT: 0x80,
      SHOW_DOCUMENT: 0x100,
      SHOW_DOCUMENT_TYPE: 0x200,
      SHOW_DOCUMENT_FRAGMENT: 0x400,
      SHOW_NOTATION: 0x800,
      FILTER_ACCEPT: 1,
      FILTER_REJECT: 2,
      FILTER_SKIP: 3
    };
  }
  if (typeof G.NodeIterator === "undefined") {
    G.NodeIterator = function NodeIterator() {
      this.root = null;
      this.nextNode = function () { return null; };
      this.previousNode = function () { return null; };
      this.detach = function () {};
    };
  }
  if (typeof G.NodeList === "undefined") {
    G.NodeList = function NodeList() {};
  }
  if (typeof G.HTMLCollection === "undefined") {
    G.HTMLCollection = function HTMLCollection() {};
  }
  if (typeof G.CSSStyleDeclaration === "undefined") {
    G.CSSStyleDeclaration = function CSSStyleDeclaration() {};
  }
  if (typeof G.DOMTokenList === "undefined") {
    G.DOMTokenList = function DOMTokenList() {};
  }
  if (typeof G.MutationObserver === "undefined") {
    G.MutationObserver = function MutationObserver(cb) {
      this.observe = function () {};
      this.disconnect = function () {};
      this.takeRecords = function () { return []; };
    };
  }
  if (typeof G.IntersectionObserver === "undefined") {
    G.IntersectionObserver = function IntersectionObserver(cb) {
      this.observe = function () {};
      this.disconnect = function () {};
      this.takeRecords = function () { return []; };
    };
  }
  if (typeof G.ResizeObserver === "undefined") {
    G.ResizeObserver = function ResizeObserver(cb) {
      this.observe = function () {};
      this.disconnect = function () {};
      this.unobserve = function () {};
    };
  }
  if (typeof G.DOMParser === "undefined") {
    G.DOMParser = function DOMParser() {
      this.parseFromString = function (str) {
        return { documentElement: null, body: null, querySelector: function () { return null; },
                 querySelectorAll: function () { return []; }, getElementById: function () { return null; } };
      };
    };
  }
  if (typeof G.Image === "undefined") {
    // 真实浏览器里 `new Image()` 返回 HTMLImageElement。站点装载器
    // (实测百度地图 DOM 渲染器 `new Image; img.onload=...; img.src=...`)
    // 依赖 onload 推进图块装载;返回纯数据对象永远等不到事件,地图停在
    // 「已建容器但没有瓦片/控件」状态。这里复用 createElement("img") 的
    // 动态元素(prelude 为其 src 提供了异步 load 语义)。
    G.Image = function Image(w, h) {
      var el = null;
      try { el = document.createElement("img"); } catch (e) { el = null; }
      if (!el) {
        el = { width: w || 0, height: h || 0, src: "", onload: null, onerror: null, complete: false,
               naturalWidth: 0, naturalHeight: 0, crossOrigin: null };
        return el;
      }
      if (w !== undefined) el.width = w;
      if (h !== undefined) el.height = h;
      return el;
    };
  }
  if (typeof G.Audio === "undefined") {
    // `new Audio(url)`:音频不渲染,但构造必须存在(实测京东首页脚本裸用
    // `new Audio(...)`,缺失即 ReferenceError 断链)。
    G.Audio = function Audio(src) {
      return {
        src: src == null ? "" : String(src),
        currentSrc: "", duration: 0, currentTime: 0, volume: 1, muted: false,
        paused: true, ended: false, loop: false, playbackRate: 1, readyState: 0,
        canPlayType: function () { return ""; },
        play: function () { this.paused = false; var p = this.onplay; if (typeof p === "function") { try { p(); } catch (e) {} } },
        pause: function () { this.paused = true; var p = this.onpause; if (typeof p === "function") { try { p(); } catch (e) {} } },
        load: function () {},
        addEventListener: function (t, fn) { this["on" + String(t)] = fn; },
        removeEventListener: function () {},
      };
    };
  }

  // --- 编码 / 加密 ---
  if (typeof G.TextEncoder === "undefined") {
    G.TextEncoder = function TextEncoder() {
      this.encoding = "utf-8";
      this.encode = function (s) {
        var str = String(s == null ? "" : s);
        var out = [];
        for (var i = 0; i < str.length; i++) {
          var c = str.charCodeAt(i);
          if (c < 0x80) out.push(c);
          else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 0x3f));
          else out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
        }
        return new Uint8Array(out);
      };
    };
  }
  if (typeof G.TextDecoder === "undefined") {
    G.TextDecoder = function TextDecoder() {
      this.encoding = "utf-8";
      this.decode = function (bytes) {
        if (!bytes) return "";
        var arr = bytes.length !== undefined ? bytes : [];
        var out = "";
        for (var i = 0; i < arr.length; i++) out += String.fromCharCode(arr[i]);
        try { return decodeURIComponent(escape(out)); } catch (e) { return out; }
      };
    };
  }
  if (typeof G.crypto === "undefined") {
    G.crypto = {
      getRandomValues: function (arr) {
        for (var i = 0; i < arr.length; i++) arr[i] = Math.floor(Math.random() * 256);
        return arr;
      },
      randomUUID: function () {
        var s = "";
        for (var i = 0; i < 32; i++) s += "0123456789abcdef".charAt(Math.floor(Math.random() * 16));
        return s.slice(0, 8) + "-" + s.slice(8, 12) + "-" + s.slice(12, 16) + "-" + s.slice(16, 20) + "-" + s.slice(20);
      }
    };
  }

  // --- 表单 / 二进制 ---
  if (typeof G.FormData === "undefined") {
    G.FormData = function FormData() {
      var pairs = [];
      this.append = function (k, v) { pairs.push([String(k), String(v)]); };
      this.set = function (k, v) {
        k = String(k);
        for (var i = 0; i < pairs.length; i++) if (pairs[i][0] === k) { pairs[i][1] = String(v); return; }
        pairs.push([k, String(v)]);
      };
      this.get = function (k) {
        k = String(k);
        for (var i = 0; i < pairs.length; i++) if (pairs[i][0] === k) return pairs[i][1];
        return null;
      };
      this.has = function (k) { return this.get(k) !== null; };
      this.delete = function (k) {
        k = String(k);
        for (var i = 0; i < pairs.length; i++) if (pairs[i][0] === k) { pairs.splice(i, 1); i--; }
      };
      this.forEach = function (fn) { for (var i = 0; i < pairs.length; i++) fn(pairs[i][1], pairs[i][0]); };
      this.entries = function () { return pairs.slice(); };
    };
  }
  if (typeof G.Blob === "undefined") {
    G.Blob = function Blob(parts, opts) {
      opts = opts || {};
      this.type = String(opts.type || "");
      this.size = 0;
    };
  }
  if (typeof G.FileReader === "undefined") {
    G.FileReader = function FileReader() {
      this.onload = null;
      this.onerror = null;
      this.result = null;
      this.readyState = 0;
      this.readAsText = function () { this.readyState = 2; if (this.onload) this.onload({ target: this }); };
      this.readAsDataURL = function () { this.readyState = 2; if (this.onload) this.onload({ target: this }); };
      this.readAsArrayBuffer = function () { this.readyState = 2; if (this.onload) this.onload({ target: this }); };
      this.abort = function () {};
    };
  }

  // --- 调度 / 工具 ---
  if (typeof G.structuredClone === "undefined") {
    G.structuredClone = function (v) {
      try { return JSON.parse(JSON.stringify(v)); } catch (e) { return v; }
    };
  }
  if (typeof G.queueMicrotask === "undefined") {
    G.queueMicrotask = function (fn) { try { fn(); } catch (e) {} };
  }
  if (typeof G.requestIdleCallback === "undefined") {
    G.requestIdleCallback = function (fn) {
      try { fn({ didTimeout: false, timeRemaining: function () { return 16; } }); } catch (e) {}
      return 1;
    };
    G.cancelIdleCallback = function () {};
  }
  if (typeof G.Intl === "undefined") {
    G.Intl = { DateTimeFormat: function () { return { format: function () { return ""; } }; },
               NumberFormat: function () { return { format: function () { return ""; } }; } };
  }

// --- AMD/CommonJS 装载器(最小同步实现) ---
// 站点脚本普遍用 requirejs 装载模块;缺失 `require` 会直接 ReferenceError,
// 页面主逻辑一行都跑不起来(实测百度地图的 widget 层整层不加载)。
// 这里按"够用即可"实现同步装载:解析模块 URL → 取回源码 → 同步求值 → 缓存导出。
(function () {
  if (typeof globalThis === "undefined") return;
  var G = globalThis;
  if (G.define && G.define.amd) return; // 已有实现不覆盖
  var cfg = { baseUrl: "/", paths: {}, packages: {}, shim: {} };
  var resMap = {};
  var pkgMap = {};
  var modules = {};
  var defs = {};
  var loadStack = [];

  function amdLog(msg) {
    try {
      if (typeof __cerror === "function") __cerror(String(msg));
      else if (G.console && G.console.error) G.console.error(String(msg));
    } catch (e) {}
  }
  function isAbsolute(u) { return /^([a-zA-Z][a-zA-Z0-9+.-]*:)?\/\//.test(String(u)); }
  function joinUrl(base, rel) {
    rel = String(rel == null ? "" : rel);
    if (isAbsolute(rel)) return rel;
    var b = String(base == null ? "/" : base);
    if (b.charAt(b.length - 1) !== "/") b = b.replace(/[^/]*$/, "") + "/";
    if (rel.charAt(0) === "/") return rel;
    return b + rel;
  }
  function normalizeId(id, baseId) {
    var s = String(id == null ? "" : id);
    if (s.charAt(0) !== ".") return s;
    var parts = String(baseId || "").split("/");
    parts.pop();
    var segs = s.split("/");
    for (var i = 0; i < segs.length; i++) {
      var sg = segs[i];
      if (sg === "." || sg === "") continue;
      if (sg === "..") parts.pop();
      else parts.push(sg);
    }
    return parts.join("/");
  }
  function resolveUrl(id) {
    var r = resMap[id];
    if (r && r.url) return joinUrl(cfg.baseUrl, r.url);
    // pkg 打包映射:模块捆绑在包文件里(resourceMap 的 res[].pkg → pkg[].url),
    // 真实 requirejs 语义是加载包 URL(百度地图 439 个模块里大量走 pkg)。
    if (r && r.pkg && pkgMap[r.pkg] && pkgMap[r.pkg].url) {
      return joinUrl(cfg.baseUrl, pkgMap[r.pkg].url);
    }
    var segs = String(id).split("/");
    var head = segs[0];
    var rest = String(id).slice(head.length);
    if (cfg.paths && cfg.paths[head]) {
      return joinUrl(cfg.baseUrl, String(cfg.paths[head]).replace(/^\//, "") + rest);
    }
    if (cfg.packages && cfg.packages[head] && cfg.packages[head].url) {
      var pu = String(cfg.packages[head].url);
      return joinUrl(cfg.baseUrl, pu.replace(/\/$/, "") + rest + ".js");
    }
    // id 本身常以 .js 结尾(common:widget/xxx.js),不能再拼一次。
    if (/\.js(\?|$)/.test(String(id))) return joinUrl(cfg.baseUrl, String(id));
    return joinUrl(cfg.baseUrl, String(id) + ".js");
  }
  function fetchTextSync(url) {
    if (typeof __lsmFetchNative !== "function") return "";
    var raw = "";
    try {
      var o = { method: "GET" };
      if (typeof __lsmCookie === "string") o.cookies = __lsmCookie;
      raw = __lsmFetchNative(String(url), JSON.stringify(o));
    } catch (e) { return ""; }
    var r = {};
    try { r = JSON.parse(raw); } catch (e) { return ""; }
    if (r.error) { amdLog("模块取回失败 " + url + ": " + r.error); return ""; }
    try {
      if (typeof __lsmCookieApply === "function" && r.setCookie && r.setCookie.length) __lsmCookieApply(r.setCookie);
    } catch (e) {}
    return r.body == null ? "" : String(r.body);
  }
  function define(name, deps, factory) {
    var id = null, d = deps, f = factory;
    if (typeof name === "function") { f = name; d = []; }
    else if (typeof deps === "function") { f = deps; d = []; }
    else if (typeof name === "string") { id = name; d = (typeof deps === "string") ? [deps] : (deps || []); }
    else { d = deps || []; }
    if (!id) id = loadStack.length ? loadStack[loadStack.length - 1] : null;
    var rec = { id: id, deps: d || [], factory: f };
    if (id) { defs[id] = rec; }
    return rec;
  }
  define.amd = {};
  function loadModule(id, baseId) {
    var mid = normalizeId(id, baseId);
    if (Object.prototype.hasOwnProperty.call(modules, mid) && modules[mid] !== undefined) return modules[mid];
    var rec = defs[mid];
    if (!rec) {
      var url = resolveUrl(mid);
      var src = fetchTextSync(url);
      if (src) {
        loadStack.push(mid);
        // 空页面初始不应暴露 AMD：UMD 会据此决定注册模块还是浏览器全局。
        // 仅在 require() 的内部取模块求值期间提供兜底 define；若脚本装了自己的
        // AMD 装载器（G.define 已不再是本兜底函数），则保留页面实现。
        var previousDefine = G.define;
        var installedDefine = typeof previousDefine !== "function";
        if (installedDefine) G.define = define;
        try { (0, eval)(src); }
        catch (e) { amdLog("模块 " + mid + " 执行失败: " + String((e && e.stack) || e)); }
        if (installedDefine && G.define === define) {
          if (previousDefine === undefined) { try { delete G.define; } catch (e2) { G.define = undefined; } }
          else G.define = previousDefine;
        }
        loadStack.pop();
      }
      rec = defs[mid];
    }
    var exports = {};
    if (rec && typeof rec.factory === "function") {
      var args = [];
      var deps = rec.deps || [];
      var moduleObj = { exports: exports };
      for (var i = 0; i < deps.length; i++) {
        var depId = normalizeId(deps[i], mid);
        // AMD/CommonJS 特殊依赖:真实 requirejs 会注入这三个标识符,
        // 站点模块普遍 `function(require, exports, module)` 三参工厂
        // (百度 tangram/messageCenter 整层如此),不注入的话 exports 参数
        // 是 undefined,`exports.foo = ...` 直接抛错或写进丢弃对象,全局永不导出。
        if (depId === "require") { args.push(requireFn); continue; }
        if (depId === "exports") { args.push(exports); continue; }
        if (depId === "module") { args.push(moduleObj); continue; }
        args.push(loadModule(depId, mid));
      }
      // deps 缺省的三参工厂(define(id, function(require, exports, module))):
      // 按 AMD 约定补注入(CommonJS 风格模块没有依赖数组)。
      if (!deps.length && rec.factory.length >= 1) {
        args = [requireFn, exports, moduleObj];
      }
      var ret;
      try { ret = rec.factory.apply(null, args); }
      catch (e) { amdLog("模块 " + mid + " 工厂执行失败: " + String((e && e.stack) || e)); }
      // 导出合并优先级:工厂返回值 > module.exports 写入 > 预置空对象。
      if (ret !== undefined && ret !== null) exports = ret;
      else if (moduleObj.exports && moduleObj.exports !== exports) exports = moduleObj.exports;
    }
    modules[mid] = exports;
    return exports;
  }
  function requireFn(deps, cb, errCb) {
    if (typeof deps === "string") return loadModule(deps, null);
    if (typeof deps === "function") { cb = deps; deps = []; }
    var list = deps || [];
    var out = [];
    for (var i = 0; i < list.length; i++) out.push(loadModule(list[i], null));
    if (typeof cb === "function") { try { cb.apply(null, out); } catch (e) { amdLog("require 回调失败: " + String((e && e.stack) || e)); } }
    return out.length === 1 ? out[0] : out;
  }
  requireFn.config = function (c) {
    c = c || {};
    if (c.baseUrl) cfg.baseUrl = String(c.baseUrl);
    if (c.paths) for (var k in c.paths) if (Object.prototype.hasOwnProperty.call(c.paths, k)) cfg.paths[k] = c.paths[k];
    if (c.packages) for (var k2 in c.packages) if (Object.prototype.hasOwnProperty.call(c.packages, k2)) cfg.packages[k2] = c.packages[k2];
    if (c.shim) for (var k3 in c.shim) if (Object.prototype.hasOwnProperty.call(c.shim, k3)) cfg.shim[k3] = c.shim[k3];
    return requireFn;
  };
  requireFn.resourceMap = function (m) {
    if (m && m.res) for (var k in m.res) if (Object.prototype.hasOwnProperty.call(m.res, k)) resMap[k] = m.res[k];
    if (m && m.pkg) for (var p in m.pkg) if (Object.prototype.hasOwnProperty.call(m.pkg, p)) pkgMap[p] = m.pkg[p];
    return requireFn;
  };
  requireFn.defined = function (id) { return Object.prototype.hasOwnProperty.call(modules, id); };
  requireFn.specified = function (id) { return Object.prototype.hasOwnProperty.call(defs, id) || Object.prototype.hasOwnProperty.call(resMap, id); };
  requireFn.undef = function (id) { delete modules[id]; delete defs[id]; };
  requireFn.toUrl = function (id) { return resolveUrl(id); };
  // `require.async(deps, cb)`:百度系(Tangram/FIS)普遍用它启动 widget 层
  // (实测 initmap 的 require.async([...tangram, statCode, init]) —— 缺它直接
  // "not a callable function",widget 层整层不启动)。真实浏览器里是异步装载,
  // 这里同步执行同一装载路径,cb 立刻触发,时序差异由宿主轮次兜底。
  requireFn.async = function (deps, cb, errCb) { return requireFn(deps, cb, errCb); };

  if (typeof G.require === "undefined") G.require = requireFn;
  // 刻意**不**定义 `requirejs`:站点自带的 requirejs 在初始化时有一句
  // `if ("undefined" != typeof requirejs) { if (isFunction(requirejs)) return; ... }`
  // ——若内核抢先定义了 `requirejs`,页面自带的 requirejs 会认为"已加载过"而
  // 直接 return,AMD 装载器整层不初始化(实测百度地图的 widget 层即因此不加载)。
  // 不预置全局 define：真实空页面没有 AMD。兜底 define 只在 loadModule 的
  // 内部求值期间临时安装，避免 UMD 在站点装载器执行前就被截走浏览器全局导出。
})();

  // performance 裸全局(部分脚本直接用 `performance.now()`)。
  if (typeof G.performance === "undefined" && G.window && G.window.performance) {
    G.performance = G.window.performance;
  }
})();
"##;
