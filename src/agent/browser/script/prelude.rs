//! PRELUDE v2:JS 线程内的页面环境(持久全局)。
//!
//! 由 Rust 在 Context 创建后一次性求值;之后每次请求经 `__lsmRefresh` 注入最新
//! DOM 快照与页面元信息。与阶段一快照桥(v1)的差异见
//! `docs/自定义浏览器和V8引擎/05-阶段二页面脚本与动态DOM设计.md` 第四节。

/// 原生函数由 Rust 注册(`__lsmFetchNative`/`atob`/`btoa`),此处只做 JS 侧装配。
const PRELUDE_BODY: &str = r##"
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
// 动态插入的 <script> 的 load/error 回调(AMD 装载器靠它推进模块解析)。
var __lsmScriptCbs = {};
// 线程内已同步装载过的动态脚本 URL(去重:同一 URL 只装一次)。
var __lsmDynLoaded = {};
var __lsmTimerSeq = 0;
var __lsmNow = 0;
var __lsmTimerRan = 0;
function __lsmSetCurrentScript(o) { globalThis.__lsmCurrentScript = String(o || ""); }
var __lsmFatal = null;
var __lsmDispatchResult = { handled: false, defaultPrevented: false };
// 阶段五第一步:观察者系列(MutationObserver/IntersectionObserver/ResizeObserver)。
// 站点前端框架普遍靠 MutationObserver 驱动二次渲染,桩实现(回调永不触发)会让
// 页面停在「等 DOM 变化」状态;这里按标准语义真实派发(见 17 号设计文档)。
var __lsmMutObservers = [];
var __lsmMutQueue = [];
var __lsmIObservers = [];
var __lsmRObservers = [];
// 上一轮快照索引:宿主侧变更 diff 的基线(Agent 动作造成的 DOM 变化对页面脚本可见)。
var __lsmPrevSnap = null;
// 动态节点自增 uid:快照节点每轮重建包装器,对象同一性跨轮不可用,一律按键比较。
var __lsmUidSeq = 0;
var __lsmFlushDepth = 0;
// 变更序号:观察者只应看到登记之后的变更(seq 守卫)。
var __lsmMutSeq = 0;
// DOM 代号:任何结构性改动(append/remove/innerHTML/快照重建)都 +1。
// 活集合据此判定缓存是否过期(见 __lsmLive)。
var __lsmDomGen = 0;
// ---------- 动态节点几何回填表 ----------
// 站点地图/路线面板的容器由 JS 建出(document.createElement + 设尺寸 + appendChild),
// 这些节点不在服务端 HTML 里,旧实现把 clientWidth/offsetWidth 写死为 0,
// 地图读到 0×0 就不装载(实测百度地图停在骨架态:0 个 canvas、body 下只剩一个薄盒)。
// 宿主其实**已经**把这些节点重新解析进 DOM 并参与布局,几何就在快照的 rect 里;
// 这里按 `data-lsm-id` 把几何认回 JS 侧的动态节点,纯 JS 侧闭环,宿主零改动。
//   __lsmDynGeom[key] = {x,y,w,h}
//   __lsmDynById[key]  = 动态节点对象(供事件派发按 key 反查)
var __lsmDynGeom = {};
var __lsmDynById = {};
function __lsmDynKey(el) {
  if (!el) return null;
  if (el.__lsmUid != null) return "d" + el.__lsmUid;
  return null;
}
function __lsmDynGeo(el) {
  var k = __lsmDynKey(el);
  var g = k && __lsmDynGeom[k];
  if (g && typeof g.w === "number") return g;
  // 未挂载(宿主还没 layout 到)按 display:none 语义取 0。
  return { x: 0, y: 0, w: 0, h: 0 };
}
function __lsmDynRect(el) {
  var g = __lsmDynGeo(el);
  return { x: g.x, y: g.y, top: g.y, left: g.x,
           right: g.x + g.w, bottom: g.y + g.h, width: g.w, height: g.h };
}

// ---------- 动态节点「挂载后」属性回写 ----------
//
// 动态节点挂载那一刻,宿主只拿到「序列化当时的属性值」(`__serEl` → append_html);
// 站点此后再改 style/class/src/显隐,旧实现**只改本地 __attrs**,不产生宿主动作,
// 于是宿主 DOM 永远停在挂载时的样子。
// 典型症状:百度地图瓦片 `<img>` 挂载时 `opacity:0`(入场淡入动画的初始态),
// 站点在 load 之后把 opacity 改成 1 → 变更到不了宿主 → 瓦片永远透明 →
// 地图区域整片空白(实测 80/81 张瓦片 opacity:0,其中 30 张几何就在视口内)。
//
// 这里把「挂载后的属性变更」按元素合并成一组 set_attr/remove_attr 动作,
// 选择器用 `[data-lsm-id="d<uid>"]`(与 __lsmPushDynChild 同款定位)。
// 挂载**前**写的属性不记:那些值会随序列化一并进宿主,重复回写只会浪费动作。
var __lsmPendingAttrs = {};
function __lsmMarkMounted(el) {
  if (!el || el.__mounted) return;
  el.__mounted = true;
  var kids = el.__kids || [];
  for (var i = 0; i < kids.length; i++) __lsmMarkMounted(kids[i]);
}
function __lsmRecordAttr(el, name, value) {
  if (!el || !el.__mounted) return;
  var key = __lsmDynKey(el);
  if (!key) return;
  var p = __lsmPendingAttrs[key];
  if (!p) { p = { set: {}, del: [] }; __lsmPendingAttrs[key] = p; }
  p.set[name] = value;
  var di = p.del.indexOf(name);
  if (di >= 0) p.del.splice(di, 1);
}
function __lsmRecordAttrRemove(el, name) {
  if (!el || !el.__mounted) return;
  var key = __lsmDynKey(el);
  if (!key) return;
  var p = __lsmPendingAttrs[key];
  if (!p) { p = { set: {}, del: [] }; __lsmPendingAttrs[key] = p; }
  delete p.set[name];
  if (p.del.indexOf(name) < 0) p.del.push(name);
}
// 每轮收口前合并下发:同名属性取最终值,每元素每属性每轮只推一条。
function __lsmFlushDynAttrs() {
  var keys = Object.keys(__lsmPendingAttrs);
  for (var i = 0; i < keys.length; i++) {
    var key = keys[i];
    var p = __lsmPendingAttrs[key];
    var sel = '[data-lsm-id="' + key + '"]';
    var names = Object.keys(p.set);
    for (var j = 0; j < names.length; j++) {
      __push({ type: "set_attr", selector: sel, key: names[j], value: String(p.set[names[j]]) });
    }
    for (var k = 0; k < p.del.length; k++) {
      __push({ type: "remove_attr", selector: sel, key: p.del[k] });
    }
  }
  __lsmPendingAttrs = {};
}
// __attrs 的写拦截:属性变更一律记账(挂载后才推宿主)。
function __mkDynAttrs(el) {
  var store = {};
  if (typeof Proxy !== "function") return store;
  return new Proxy(store, {
    get: function (t, k) { return t[k]; },
    set: function (t, k, v) {
      t[k] = v;
      var name = String(k);
      if (name === "data-lsm-id") return true;
      __lsmRecordAttr(el, name, v);
      return true;
    },
    deleteProperty: function (t, k) {
      delete t[k];
      __lsmRecordAttrRemove(el, String(k));
      return true;
    },
    defineProperty: function (t, k, desc) {
      if (desc && Object.prototype.hasOwnProperty.call(desc, "value")) {
        t[k] = desc.value;
        var name = String(k);
        if (name !== "data-lsm-id") __lsmRecordAttr(el, name, desc.value);
      } else {
        Object.defineProperty(t, k, desc);
        __lsmRecordAttrRemove(el, String(k));
      }
      return true;
    }
  });
}

var MAX_ACTIONS = 4096;
var MAX_CONSOLE = 2048;

function __push(a) { if (__lsmActions.length < MAX_ACTIONS) __lsmActions.push(a); }
function __clog(level, text) {
  if (__lsmConsole.length < MAX_CONSOLE) __lsmConsole.push({ level: level, text: String(text) });
}
function __cerror(text) { __clog("error", text); }

// ---------- Annex B 兼容:String.prototype.substr ----------
// Boa 0.20 默认关闭 annex-b,`substr` 缺失;站点会自带补丁,但实测百度地图里
// 落到一个 native 形态的实现上,`"array".substr(1)` 直接抛
// "cannot convert 'null' or 'undefined' to object",tangram 初始化第一条
// `createChain` 即被打断。这里按 Annex B.2.2.1 提供正确实现,并在每轮异常时
// 重新断言(页面脚本可能覆盖成坏实现)。
function __lsmSubstrPolyfill(start, length) {
  var s = String(this);
  var size = s.length;
  var begin = Number(start);
  if (begin !== begin) begin = 0;
  else if (begin === Infinity) begin = size;
  else if (begin === -Infinity) begin = 0;
  else {
    begin = begin < 0 ? Math.ceil(begin) : Math.floor(begin);
    begin = begin < 0 ? Math.max(size + begin, 0) : Math.min(begin, size);
  }
  var len;
  if (length === undefined) len = size;
  else {
    len = Number(length);
    if (len !== len || len <= 0) len = 0;
    else if (len === Infinity) len = size - begin;
    else len = Math.min(Math.floor(len), size - begin);
  }
  return s.substring(begin, begin + len);
}
__lsmSubstrPolyfill.__lsmPolyfill = true;
function __lsmEnsureSubstr() {
  var cur = String.prototype.substr;
  if (typeof cur === "function" && cur.__lsmPolyfill) return;
  try {
    Object.defineProperty(String.prototype, "substr", {
      value: __lsmSubstrPolyfill,
      writable: true,
      enumerable: false,
      configurable: true
    });
  } catch (e) {}
}
__lsmEnsureSubstr();
// 调试:记录谁在运行期覆盖了 substr(仅有 LAEW_JS_TRACE_TOOBJ 时安装)。
if (globalThis.__lsmTraceToObjInstall) {
  try {
    var __lsmSubstrVal = String.prototype.substr;
    Object.defineProperty(String.prototype, "substr", {
      configurable: true,
      enumerable: false,
      get: function () { return __lsmSubstrVal; },
      set: function (v) {
        try { __cerror("[substr] 被覆盖 -> " + String(v).slice(0, 120)); } catch (e) {}
        __lsmSubstrVal = v;
      }
    });
  } catch (e) {}
}

function __lsmRefresh(nodes, meta) {
  __lsmEnsureSubstr();
  // 宿主侧变更 diff:与上一轮快照比对,合成 mutation 记录。
  // Agent 侧动作(input_text 写值、click 后 DOM 重建)只体现在快照里,
  // 页面脚本若无通知就只能轮询;这里把差异翻译成标准 mutation 记录喂给观察者。
  // 没有观察者时不算 diff(大页面每请求一次全量比对不划算)。
  var next = nodes || [];
  var diff = [];
  if (__lsmActiveObservers() > 0 || __lsmPrevSnap) {
    var nextIdx = __lsmSnapIndex(next);
    diff = __lsmDiffSnap(__lsmPrevSnap, nextIdx);
    __lsmPrevSnap = nextIdx;
  }
  // 动态节点几何回填 + 收养:快照里带 `data-lsm-id` 的节点,就是上一轮 JS 建出来
  // 的那批。把 rect 还给它,并把动态对象绑到快照节点上 —— 之后宿主点击落在该
  // 快照节点时,注册在动态对象上的监听也能收到(见 __fire 的 dynChain)。
  for (var di = 0; di < next.length; di++) {
    var dn = next[di];
    var dk = dn && dn.attrs ? dn.attrs["data-lsm-id"] : null;
    if (!dk) continue;
    if (dn.rect && typeof dn.rect.length === "number" && dn.rect.length >= 4) {
      __lsmDynGeom[dk] = {
        x: Math.round(Number(dn.rect[0]) || 0),
        y: Math.round(Number(dn.rect[1]) || 0),
        w: Math.max(0, Math.round(Number(dn.rect[2]) || 0)),
        h: Math.max(0, Math.round(Number(dn.rect[3]) || 0))
      };
    }
    var dyn = __lsmDynById[dk];
    if (dyn) {
      dyn.__lsm_node = dn;
      dyn.__lsm_idx = dn.i;
      dyn.__detached = false;
    }
  }
  __lsmNodes = next;
  __lsmDomGen++;
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
  window.__lsmPrefersDark = !!__lsmMeta.prefersDark;
  window.__lsmTraceToObj = !!__lsmMeta.traceToObj;
  window.__lsmTraceStmt = !!__lsmMeta.traceStmt;
  window.scrollX = Number(__lsmMeta.scrollX) || 0;
  window.scrollY = Number(__lsmMeta.scrollY) || 0;
  window.pageXOffset = window.scrollX;
  window.pageYOffset = window.scrollY;
  __rebindDocNodes();
  __rebindLocation();
  __lsmApplyDiff(diff);
  // 注意:这里刻意**不**重置 document.readyState。refresh 只是注入最新 DOM 快照,
  // 页面生命周期状态属于整个页面会话:管线尾部已依序置 interactive/complete,
  // 若每次请求(eval_js/click/input 都会 refresh)都打回 "loading",页面按
  // readyState 分支的初始化会全部误判「还在加载」(实测百度地图即如此)。
  // 导航换页会重建 JS 线程,新 Context 天然从 "loading" 开始。
}

// 快照节点身份:跨轮次必须稳定。
//
// 快照用数组下标标识节点,但 DOM 会在两轮之间变化(站点脚本插入/删除节点),
// 下标随之错位:上一轮捕获的元素包装器会指向**别的**元素。百度地图首屏就是
// 这么断的——`find(".right-button")` 落空 → `offset()` 返回 undefined →
// 读 `s.left` 抛 TypeError → 搜索框整块初始化失败、地图不出图。
// 真实浏览器的元素身份与 DOM 变化无关,这里按**稳定选择器**重新定位。
function __lsmCurrentIdx(n) {
  if (!n) return -1;
  var sel = String(n.sel || "");
  if (sel) {
    for (var i = 0; i < __lsmNodes.length; i++) {
      if (String(__lsmNodes[i].sel || "") === sel) return i;
    }
  }
  var idx = Number(n.i);
  return (isFinite(idx) && idx >= 0 && idx < __lsmNodes.length) ? idx : -1;
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

// 文本节点落到宿主 DOM:站点用 `document.createTextNode(css)` 往 <style> 里塞 CSS
// (百度地图 require.loadCss 的注入方式)。此前 appendChild 只认「detached 元素」和
// 「快照节点」两种,文本节点两个分支都不落 → 运行时注入的 CSS 全部丢失,
// 站点动态样式缺失 → 布局与真实浏览器对不上。
function __lsmPushText(n, c) {
  var txt = String(c.textContent != null ? c.textContent : (c.data != null ? c.data : ""));
  var key = n.sel || ("#idx_" + n.i);
  var prev = Object.prototype.hasOwnProperty.call(__lsmText, key) ? __lsmText[key] : (n.text || "");
  __lsmText[key] = String(prev) + txt;
  // append_text 而非 set_text:浏览器的 appendChild(textNode) 是**追加**,
  // set_text 会把元素既有子节点全换掉(误删原有内容)。
  __push({ type: "append_text", selector: n.sel, value: txt });
}

// 动态节点**挂载之后**再追加子节点,必须回写宿主 DOM。
//
// 动态节点在宿主里以 `data-lsm-id` 定位(见 `__serEl`),站点却常在挂载之后
// 才往里塞内容:百度地图先建 `platform` 挂到地图容器,再把图层面板
// (`overlayDiv` / `mapPane` / 瓦片)追加进去。旧实现只在「动态节点被挂到快照
// 节点」那一刻推一次宿主动作,之后动态节点内部的变更**只改本地树**,宿主 DOM
// 永远看不到 —— 地图面板与瓦片因此进不了页面(实测 `map._panes.mapPane` 已存在,
// 而页面里 `overlayDiv` 数量为 0)。
function __lsmPushDynChild(el, child, before) {
  try {
    if (!el || !child) return;
    var key = __lsmDynKey(el);
    if (!key) return;
    var html = __serEl(child);
    if (!html) return;
    var sel = '[data-lsm-id="' + key + '"]';
    __push({ type: before ? "insert_html" : "append_html", selector: sel, value: html });
    // 进宿主了:子树上的属性变更从此要回写宿主(见 __lsmRecordAttr)。
    __lsmMarkMounted(child);
  } catch (e) {}
}

// 动态节点的整棵子树替换(清空 + 重填)。
function __lsmPushDynContent(el) {
  try {
    if (!el) return;
    var key = __lsmDynKey(el);
    if (!key) return;
    var sel = '[data-lsm-id="' + key + '"]';
    var html = "";
    var kids = el.__kids || [];
    for (var i = 0; i < kids.length; i++) html += __serEl(kids[i]);
    if (el.__pendingHtml) html = el.__pendingHtml + html;
    __push({ type: "set_html", selector: sel, value: html });
  } catch (e) {}
}

// ---------- HTMLCollection:活集合 ----------
// 真实浏览器的 getElementsByTagName / getElementsByClassName 返回**实时集合**:
// 「取集合 → 改 DOM → 再读集合」,集合内容随 DOM 变化。
// Lsm 曾返回一次性数组快照,站点当场读到 undefined 而炸链。
// 实测百度地图:`mod_5dbb339.js` 的 `require.loadCss` → 内部 CSS 注入函数
//   var n=head.getElementsByTagName("style");
//   if(!n.length){ 创建 <style> 并 appendChild }
//   var o=n[0]; o.getAttribute("media")
// 快照期 head 里还没有 <style>(宿主动作尚未回写),走创建分支;而 n 是静态数组,
// appendChild 之后 n[0] 仍是 undefined → `o.getAttribute` 抛 TypeError
// → `common:widget/ui/dataCopyRight/dataCopyRight.js` 模块工厂失败
// → setCurCity → init → setStatus 整条地图初始化链断掉
// → 地图不出图、路线面板不出、页面只剩搜索框。
//
// 实现:Proxy 包裹一个数组,`length`/数字下标/`item()`/`namedItem()`/迭代/`Object.keys`
// 全部**实时重查** collect();无 Proxy 环境(理论上不会发生)回退成普通数组快照。
function __lsmLive(collect) {
  if (typeof Proxy !== "function") return collect();
  var METHODS = ["forEach", "map", "filter", "slice", "indexOf", "lastIndexOf", "join",
    "some", "every", "find", "findIndex", "includes", "concat", "reduce", "reduceRight",
    "sort", "reverse", "at"];
  var target = [];
  // 结果按 DOM 代号缓存:结构性改动(__lsmRec / 快照重建)后代号前进,缓存失效。
  // 避免每次读 length/下标都重跑整棵树查询(大页面上很贵)。
  var cache = { gen: -1, arr: null };
  function current() {
    if (cache.arr === null || cache.gen !== __lsmDomGen) {
      cache.arr = collect();
      cache.gen = __lsmDomGen;
    }
    return cache.arr;
  }
  return new Proxy(target, {
    get: function (t, k) {
      var arr = current();
      if (k === "length") return arr.length;
      if (k === "item") {
        return function (i) {
          var a = current();
          var x = a[Number(i)];
          return x === undefined ? null : x;
        };
      }
      if (k === "namedItem") {
        return function (nm) {
          var a = current();
          for (var i = 0; i < a.length; i++) {
            var e = a[i];
            if (!e) continue;
            if (e.id === nm) return e;
            try { if (e.getAttribute && e.getAttribute("name") === nm) return e; } catch (x) {}
          }
          return null;
        };
      }
      if (typeof k === "symbol") {
        if (k === Symbol.iterator) {
          return function () { var a = current(); return a[Symbol.iterator](); };
        }
        return undefined;
      }
      if (typeof k === "string") {
        if (/^\d+$/.test(k)) return arr[Number(k)];
        if (METHODS.indexOf(k) >= 0) {
          return function () { var a = current(); return Array.prototype[k].apply(a, arguments); };
        }
        if (k === "toString") return function () { return "[object HTMLCollection]"; };
      }
      return undefined;
    },
    has: function (t, k) {
      if (typeof k === "string" && /^\d+$/.test(k)) return Number(k) < current().length;
      return false;
    },
    ownKeys: function (t) {
      var arr = current();
      var keys = [];
      for (var i = 0; i < arr.length; i++) keys.push(String(i));
      keys.push("length");
      return keys;
    },
    getOwnPropertyDescriptor: function (t, k) {
      var arr = current();
      // 「length」在目标数组上是非配置自有属性,报成 writable 才能合法改值
      // (Proxy 不变式:不可写属性的值必须 SameValue)。
      if (k === "length") {
        return { value: arr.length, writable: true, enumerable: false, configurable: false };
      }
      if (typeof k === "string" && /^\d+$/.test(k)) {
        var idx = Number(k);
        if (idx < arr.length) {
          return { value: arr[idx], writable: false, enumerable: true, configurable: true };
        }
      }
      return undefined;
    }
  });
}

// ---------- 快照节点序列化(快照 DOM 子树 → HTML 字符串) ----------
// innerHTML / outerHTML 要给的是**HTML**,不是可见文本。
// 快照里每个节点带自己的可见文本(父节点的是后代文本之和),所以只取
// 「叶子节点的文本」,父节点不再重复输出,避免文本翻倍。
function __serSnapNode(n) {
  var tag = String((n && n.tag) || "div");
  var attrs = (n && n.attrs) || {};
  var a = [];
  for (var k in attrs) {
    if (!Object.prototype.hasOwnProperty.call(attrs, k)) continue;
    if (k === "outer_html") continue;
    a.push(" " + k + "=\"" + String(attrs[k]).replace(/"/g, "&quot;") + "\"");
  }
  var inner = "";
  var hasChild = false;
  for (var i = 0; i < __lsmNodes.length; i++) {
    if (__lsmNodes[i].parent === n.i) {
      hasChild = true;
      try { inner += __serSnapNode(__lsmNodes[i]); } catch (e) {}
    }
  }
  if (!hasChild) inner = __escHtml((n && n.text) || "");
  return "<" + tag + a.join("") + ">" + inner + "</" + tag + ">";
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
  // `data-lsm-id` 是动态节点回认宿主布局几何的钥匙:宿主解析后按属性保留
  // (common_attrs 全量收 data-*),下一轮快照带 rect 回来,JS 侧据此回填
  // clientWidth/offsetWidth(见 __lsmRefresh)。快照节点没有 __lsmUid,不发。
  var dk = __lsmDynKey(el);
  if (dk && !attrs["data-lsm-id"]) out += ' data-lsm-id="' + __escAttr(dk) + '"';
  for (var k in attrs) {
    if (!Object.prototype.hasOwnProperty.call(attrs, k)) continue;
    out += " " + k + '="' + __escAttr(attrs[k]) + '"';
  }
  out += ">";
  if (__VOID_TAGS[tag]) return out;
  // __pendingHtml:未入树元素被赋值过 innerHTML(set_html 的 @created 动作宿主无处落地),
  // 原始 HTML 挂在这里,随 append_html/insert_html 的序列化原样带到宿主再解析,
  // widget「createElement → innerHTML=模板 → appendChild」模式的内容不再丢失。
  if (el.__pendingHtml != null) out += el.__pendingHtml;
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
      // `cssText` 语义 = **整体替换**声明块(不是新增一条名为 cssText 的声明)。
      // 站点(jQuery 风格 css 助手、百度地图图层面板)大量用
      // `el.style.cssText = "position:absolute;left:...;top:..."` 一次性铺样式,
      // 旧实现把它当普通属性追加成 `cssText:position:absolute;...`,
      // 于是 `position:absolute` 失效 —— 百度地图瓦片层退化成静态块,
      // 瓦片的 left/top(以地图中心为原点)整体错位,地图区域画不出内容。
      if (String(k) === "cssText") {
        setCss(String(v == null ? "" : v));
        return true;
      }
      setCss(__cssSet(getCss(), String(k), String(v)));
      return true;
    }
  });
}

// ---------- 动态脚本的 load 事件 ----------
// 站点用 `document.createElement("script")` 插脚本后,靠脚本元素的 onload
// 回调推进后续逻辑(AMD 装载器全部依赖这个信号)。宿主执行完外链脚本后会
// 调用 `__lsmFireScriptLoad(src_url)` 触发对应回调。
function __lsmScriptKey(url) {
  var raw = String(url == null ? "" : url).trim();
  if (!raw) return raw;
  // 已是绝对地址。
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(raw)) return raw;
  // 纯字符串解析,不依赖 URL 构造器:页面脚本可能覆盖/破坏全局 URL
  // (实测百度地图环境里 new URL 抛错,归一退化成原样,load 回调再也对不上)。
  var base = String(__lsmMeta.url || "");
  var proto = String(__lsmMeta.protocol || "https:");
  var host = String(__lsmMeta.host || "");
  var origin = String(__lsmMeta.origin || (host ? proto + "//" + host : ""));
  if (raw.indexOf("//") === 0) return proto + raw;          // 协议相对
  if (raw.charAt(0) === "/") return origin + raw;            // 根相对
  if (!origin) return raw;
  // 目录相对:只取 base 的路径部分(去掉 scheme+host),再按目录拼接。
  var path = base.replace(/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\/[^\/?#]*/, "");
  var dir = path.replace(/[?#].*$/, "").replace(/[^/]*$/, "");
  return origin + dir + raw;
}
function __lsmRegisterScriptLoad(el) {
  if (!el || !el.__attrs) return;
  // 只管 <script>:img 等替换元素也带 src,误当脚本装载会把图片二进制
  // 塞进 eval 抛 SyntaxError(实测 zhipin 登录页头像/二维码)。
  var tag = String(el.__tag || el.tagName || "").toLowerCase();
  if (tag !== "script") return;
  var url = __lsmScriptKey(el.__attrs.src || "");
  if (!url) return;
  if (!__lsmScriptCbs[url]) __lsmScriptCbs[url] = [];
  __lsmScriptCbs[url].push(el);
  // 线程内同步装载:webpack/rspack 的 chunk 装载器注入 <script src> 后
  // 挂超时定时器等 onload;宿主轮次机制(序列化→收集→取回→执行→fire)
  // 要跨轮往返,虚拟时钟在等待期快进超过装载器超时阈值,chunk 被判失败、
  // 异步路由整条放弃(实测 zhipin ChunkLoadError 1326)。此处当场取回执行,
  // load 信号在回调返回前就发出。fetch 记账由原生层自动进网络台账。
  __lsmLoadDynScript(el, url);
}
function __lsmLoadDynScript(el, url) {
  url = String(url || "");
  if (!url || __lsmDynLoaded[url]) return;
  // 非 http(s) 目标(about:/data: 占位)直接当空载成功,不进网络层。
  if (!/^https?:/i.test(url)) {
    __lsmDynLoaded[url] = true;
    return;
  }
  __lsmDynLoaded[url] = true;
  var ok = false;
  try {
    var raw = __lsmFetchNative(url, JSON.stringify({ method: "GET", cookies: __lsmCookie, script: true }));
    var r = {};
    try { r = JSON.parse(raw); } catch (e2) { r = { error: "原生 fetch 返回非法结构" }; }
    if (r.setCookie && r.setCookie.length) __lsmCookieApply(r.setCookie);
    if (r.error) {
      __cerror("动态脚本取回失败 " + url + ": " + String(r.error));
    } else {
      var code = String(r.body == null ? "" : r.body);
      if (code.trim()) {
        try { (0, eval)(code); ok = true; }
        catch (e3) { __cerror("动态脚本执行失败 " + url + ": " + String((e3 && e3.stack) || e3)); ok = true; }
      } else { ok = true; }
    }
  } catch (e) { __cerror("动态脚本装载异常 " + url + ": " + String(e)); }
  // 标记已执行:宿主 collect_scripts 见 data-lsm-executed 跳过,不二次执行。
  try {
    if (el && el.__attrs) el.__attrs["data-lsm-executed"] = "1";
  } catch (e4) {}
  if (ok) __lsmFireScriptLoad(url);
  else {
    try {
      if (el && typeof el.onerror === "function") el.onerror({ type: "error", target: el });
    } catch (e5) {}
  }
}
function __lsmFireScriptLoad(url) {
  var key = __lsmScriptKey(url);
  var list = __lsmScriptCbs[key];
  if (!list || !list.length) return;
  delete __lsmScriptCbs[key];
  for (var i = 0; i < list.length; i++) {
    var el = list[i];
    try { el.readyState = "complete"; } catch (e) {}
    try {
      if (typeof el.onreadystatechange === "function") el.onreadystatechange({ type: "readystatechange", target: el });
    } catch (e) { __cerror("script onreadystatechange 失败: " + String((e && e.stack) || e)); }
    try {
      if (typeof el.onload === "function") el.onload({ type: "load", target: el });
    } catch (e) { __cerror("script onload 失败: " + String((e && e.stack) || e)); }
  }
}

// ---------- 元素工厂 ----------
// 从快照节点求几何属性组:rect 缺失时 html 兜底视口、其余取 0。
// BMap 等地图/组件库初始化读容器 clientWidth/Height,undefined 会让 NaN
// 在初始化链深处传播并炸出 ToObject TypeError(实测百度地图 widget 层即断于此)。
function __lsmGeom(n) {
  var r = n && n.rect;
  if (r && typeof r.length === "number" && r.length >= 4) {
    var x = Number(r[0]) || 0, y = Number(r[1]) || 0;
    var w = Math.max(0, Math.round(Number(r[2]) || 0));
    var h = Math.max(0, Math.round(Number(r[3]) || 0));
    return { x: Math.round(x), y: Math.round(y), w: w, h: h,
      rect: { x: x, y: y, top: y, left: x, right: x + w, bottom: y + h, width: w, height: h } };
  }
  var tag = String((n && n.tag) || "").toLowerCase();
  // html/body 没有布局盒(布局从 body 内容开始),用视口兜底:
  // 真实浏览器 document.body.clientWidth/Height 恒为布局视口尺寸,
  // 地图/全屏面板初始化直接读它,取 0 会让容器塌缩(实测百度地图)。
  if (tag === "html" || tag === "body") {
    var vw = Math.round(Number(__lsmMeta.width) || 1280);
    var vh = Math.round(Number(__lsmMeta.height) || 720);
    return { x: 0, y: 0, w: vw, h: vh,
      rect: { x: 0, y: 0, top: 0, left: 0, right: vw, bottom: vh, width: vw, height: vh } };
  }
  return { x: 0, y: 0, w: 0, h: 0,
    rect: { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0 } };
}
// 动态节点(__newEl 建出、未入快照)按「标签 + #id + .class + [attr]」做
// 宽松选择器匹配(逗号分隔,逐个或)。matches/closest 委托匹配兜底用。
function __lsmDynMatch(el, sel) {
  if (!el) return false;
  var parts = String(sel).split(",");
  for (var p = 0; p < parts.length; p++) {
    var s = parts[p].trim();
    if (!s) continue;
    // 去掉简单伪类/组合子(委托选择器如 ".a > .b" 取末段判定)。
    s = s.replace(/::?[a-z-]+(\([^)]*\))?/gi, "");
    var segs = s.split(/[\s>+~]+/);
    s = segs[segs.length - 1];
    if (!s || s === "*") return true;
    var ok = true;
    // 标签
    var m = s.match(/^[a-zA-Z][a-zA-Z0-9-]*/);
    var tag = m ? m[0].toLowerCase() : "";
    var et = String(el.__tag || el.tagName || "").toLowerCase();
    if (tag && tag !== "*" && et !== tag) ok = false;
    // #id
    var idm = s.match(/#([a-zA-Z0-9_-]+)/);
    if (ok && idm && String(el.id || "") !== idm[1]) ok = false;
    // .class
    var cls = String(el.className || "");
    var re = /\.([a-zA-Z0-9_-]+)/g;
    var cm;
    while (ok && (cm = re.exec(s))) {
      if (cls.split(/\s+/).indexOf(cm[1]) < 0) ok = false;
    }
    // [attr] / [attr="v"]
    var am;
    var reA = /\[([a-zA-Z0-9_-]+)(?:\s*=\s*["']?([^"'\]]*)["']?)?\]/g;
    while (ok && (am = reA.exec(s))) {
      var av = el.getAttribute ? el.getAttribute(am[1]) : null;
      if (av == null) ok = false;
      else if (am[2] !== undefined && am[2] !== "" && av !== am[2]) ok = false;
    }
    if (ok) return true;
  }
  return false;
}
// 同批动态 DOM 查询:宿主快照要等本轮脚本结束才回写,而 React/Vue 的
// commit/effect 会在同一批里立刻 querySelector 刚挂载的节点。只查快照会让
// 这些节点短暂「不存在」,后续 getBoundingClientRect 等调用直接 ToObject。
// 动态表按创建序遍历;已回快照收养的节点排除,避免与静态结果重复。
function __lsmQueryDyn(sel, rootEl, mountedOnly, rootSnapshotSel) {
  var out = [];
  var keys = Object.keys(__lsmDynById || {});
  for (var i = 0; i < keys.length; i++) {
    var el = __lsmDynById[keys[i]];
    if (!el || el === rootEl) continue;
    if (rootEl) {
      var scope = el;
      while (scope && scope !== rootEl) scope = scope.__parentDyn || null;
      if (scope !== rootEl) continue;
    }
    if (rootSnapshotSel) {
      var top = el;
      while (top && top.__parentDyn) top = top.__parentDyn;
      var ps = top && top.__parentStatic;
      if (!ps || String(ps.sel || "") !== String(rootSnapshotSel)) continue;
    }
    if (mountedOnly) {
      var attached = el;
      while (attached && attached.__parentDyn) attached = attached.__parentDyn;
      if (!el.__mounted || !attached || !attached.__mounted || el.__lsm_node) continue;
    }
    if (__lsmDynMatch(el, sel)) out.push(el);
  }
  return out;
}
function __lsmQueryWithDyn(sel, rootIdx) {
  var out = __lsmQuery(sel, rootIdx);
  var rootSnapshotSel = null;
  if (rootIdx != null && rootIdx >= 0 && __lsmNodes[rootIdx]) {
    rootSnapshotSel = __lsmNodes[rootIdx].sel;
  }
  return out.concat(__lsmQueryDyn(sel, null, true, rootSnapshotSel));
}
function __lsmQueryOneWithDyn(sel, rootIdx) {
  var all = __lsmQueryWithDyn(sel, rootIdx);
  return all.length ? all[0] : null;
}
// ---------- element.attributes(NamedNodeMap 近似) ----------
// 真实浏览器里每个元素都有 attributes 集合;站点库(jQuery support 检测、
// removeAttr 遍历、表单序列化)直接下标/按名访问它。此前动态节点与快照节点
// 都没有该属性,`el.attributes[name]` 在 Boa 上报 ToObject 一炸整个库死亡
// (实测 zhipin jQuery 1.12.2 的 support 分支)。
// 实现为按需构造的 Proxy 视图:每次访问实时读 attrs 容器(setAttribute
// 原地改容器,视图天然「活」),数字下标/名称下标/length/getNamedItem 全支持。
function __attrNode(ownerEl, name, value) {
  var v = value == null ? "" : String(value);
  return {
    name: String(name), nodeName: String(name), nodeType: 2,
    value: v, nodeValue: v, textContent: v,
    specified: true, expando: false, ownerElement: ownerEl || null
  };
}
function __attrMap(ownerEl, attrsRef) {
  var base = {
    getNamedItem: function (n) {
      var a = attrsRef(); n = String(n);
      return Object.prototype.hasOwnProperty.call(a, n) ? __attrNode(ownerEl, n, a[n]) : null;
    },
    setNamedItem: function (attr) {
      if (attr && attr.name != null && ownerEl && ownerEl.setAttribute) {
        ownerEl.setAttribute(String(attr.name), attr.value);
      }
      return attr || null;
    },
    removeNamedItem: function (n) {
      var old = base.getNamedItem(n);
      if (ownerEl && ownerEl.removeAttribute) ownerEl.removeAttribute(String(n));
      return old;
    },
    item: function (i) {
      var a = attrsRef(); var ks = Object.keys(a); var idx = Number(i) | 0;
      return (idx >= 0 && idx < ks.length) ? __attrNode(ownerEl, ks[idx], a[ks[idx]]) : null;
    }
  };
  return new Proxy(base, {
    get: function (t, k) {
      if (k in t) return t[k];
      var a = attrsRef();
      if (k === "length") return Object.keys(a).length;
      var ks; var idx = Number(k);
      if (typeof k === "string" && String(idx) === k) {
        ks = Object.keys(a);
        return (idx >= 0 && idx < ks.length) ? __attrNode(ownerEl, ks[idx], a[ks[idx]]) : undefined;
      }
      if (typeof k === "string" && Object.prototype.hasOwnProperty.call(a, k)) {
        return __attrNode(ownerEl, k, a[k]);
      }
      return undefined;
    },
    has: function (t, k) {
      if (k in t) return true;
      var a = attrsRef();
      if (k === "length") return true;
      if (typeof k === "string" && Object.prototype.hasOwnProperty.call(a, k)) return true;
      var idx = Number(k);
      if (typeof k === "string" && String(idx) === k) {
        return idx >= 0 && idx < Object.keys(a).length;
      }
      return false;
    }
  });
}
function __mk(n) {
  var attrs = n.attrs || {};
  // 布局几何:快照 rect [x,y,w,h] → clientWidth/offsetWidth 等属性。
  // 缺 rect 的元素取 0;html 兜底视口宽高(真实浏览器 documentElement.clientWidth 语义);
  // 动态创建元素(__newEl,未挂载)全 0——与 display:none 语义一致。
  var __geo = __lsmGeom(n);
  var el = {
    __lsm_node: n,
    __lsm_idx: n.i,
    tagName: String(n.tag || "").toUpperCase(),
    nodeName: String(n.tag || "").toUpperCase(),
    nodeType: 1,
    id: n.id || "",
    // className 经 define() 反射到 class 属性并回传宿主(见下方),
    // 此处占位避免字面量顺序问题。
    className: n.class || "",
    // ownerDocument → document(defaultView → window):标准取样式路径
    // `el.ownerDocument.defaultView.getComputedStyle(el, null)` 依赖它,
    // 缺失时该路径返回 void 0,`...getComputedStyle(t).position` 直接抛错
    // (实测百度地图 initmap._setStyle 即命中)。
    ownerDocument: null,
    name: n.name || "",
    type: n.type || "",
    title: attrs.title || "",
    alt: attrs.alt || "",
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
    clientWidth: __geo.w,
    clientHeight: __geo.h,
    offsetWidth: __geo.w,
    offsetHeight: __geo.h,
    scrollWidth: __geo.w,
    scrollHeight: __geo.h,
    offsetLeft: __geo.x,
    offsetTop: __geo.y,
    clientLeft: 0,
    clientTop: 0,
    // 滚动偏移:缺这两个值时站点的 offset() 会算出 NaN(定位全部失效)。
    scrollLeft: 0,
    scrollTop: 0,
    getAttribute: function (k) {
      k = String(k);
      return Object.prototype.hasOwnProperty.call(attrs, k) ? attrs[k] : null;
    },
    getAttributeNS: function (ns, k) { return this.getAttribute(k); },
    hasAttribute: function (k) { return Object.prototype.hasOwnProperty.call(attrs, String(k)); },
    hasAttributeNS: function (ns, k) { return this.hasAttribute(k); },
    setAttribute: function (k, v) {
      __lsmRec("attributes", el, { attributeName: String(k), attributeOldValue: __lsmAttrOld(attrs, String(k)) });
      __push({ type: "set_attr", selector: n.sel, key: String(k), value: String(v) });
      attrs[String(k)] = String(v);
    },
    setAttributeNS: function (ns, k, v) { this.setAttribute(k, v); },
    removeAttribute: function (k) {
      __lsmRec("attributes", el, { attributeName: String(k), attributeOldValue: __lsmAttrOld(attrs, String(k)) });
      __push({ type: "remove_attr", selector: n.sel, key: String(k) });
      delete attrs[String(k)];
    },
    removeAttributeNS: function (ns, k) { this.removeAttribute(k); },
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
      return __geo.rect;
    },
    scrollIntoView: function () { __push({ type: "scroll_to", selector: n.sel }); },
    scrollTo: function (x, y) { __push({ type: "scroll", value: String(y || 0) }); },
    querySelector: function (s) { return __lsmQueryOneWithDyn(s, __lsmCurrentIdx(n)); },
    // matches/closest:事件委托(jQuery `$(document).on('click', sel, fn)`、各框架
    // 委托)依赖 `el.matches()/el.closest()` 从 target 向上匹配委托选择器;
    // 缺失则委托 handler 一跑就抛,面板按钮「点击无反应」(实测百度地图路线按钮)。
    matches: function (sel) {
      var self = __lsmCurrentIdx(n);
      if (self >= 0 && __lsmMatchSel(self, String(sel))) return true;
      // 动态覆盖层节点按标签/类名做宽松匹配。
      return __lsmDynMatch(el, String(sel));
    },
    matchesSelector: function (sel) { return this.matches(sel); },
    closest: function (sel) {
      var cur = el;
      var guard = 0;
      while (cur && guard++ < 32) {
        if (cur.matches && cur.matches(String(sel))) return cur;
        cur = cur.parentNode || null;
      }
      return null;
    },
    getElementsByTagName: function (t) {
      var tag = String(t).toLowerCase();
      // 快照子树(父索引)∪ 动态覆盖层,一起按标签收集。
      // 返回**活集合**:站点「取集合 → 改 DOM → 再读集合」要能立刻看到变化
      // (百度地图 require.loadCss 的 CSS 注入器就靠这个,见 __lsmLive 注释)。
      return __lsmLive(function () {
        var out = [];
        var self = __lsmCurrentIdx(n);
        if (self < 0) return out;
        for (var i = 0; i < __lsmNodes.length; i++) {
          var nd = __lsmNodes[i];
          var anc = __ancestorChain(nd.i);
          if (anc.indexOf(self) >= 0 && (tag === "*" || String(nd.tag).toLowerCase() === tag)) out.push(__mk(nd));
        }
        for (var d = 0; d < dynKids.length; d++) {
          (function walk(k) {
            var kt = String((k.__tag || k.tagName || "")).toLowerCase();
            if (tag === "*" || kt === tag) out.push(k);
            var ks = k.__kids || [];
            for (var q = 0; q < ks.length; q++) walk(ks[q]);
          })(dynKids[d]);
        }
        return out;
      });
    },
    querySelectorAll: function (s) { return __lsmQueryWithDyn(s, __lsmCurrentIdx(n)); },
    appendChild: function (c) {
      try { __lsmRegisterScriptLoad(c); } catch (e) {}
      if (c) {
        c.__parentStatic = n;
        dynKids.push(c);
      }
      __lsmRec("childList", el, { addedNodes: c ? [c] : [] });
      if (c && c.__detached) {
        __push({ type: "append_html", selector: n.sel, value: __serEl(c) });
        __lsmMarkMounted(c);
      }
      else if (c && c.__lsm_node) __push({ type: "append_node", selector: c.__lsm_node.sel, key: n.sel });
      else if (c && c.nodeType === 3) __lsmPushText(n, c);
      return c;
    },
    insertBefore: function (c, ref) {
      try { __lsmRegisterScriptLoad(c); } catch (e) {}
      if (c) {
        c.__parentStatic = n;
        var at = dynKids.indexOf(ref);
        if (at >= 0) dynKids.splice(at, 0, c); else dynKids.push(c);
      }
      __lsmRec("childList", el, { addedNodes: c ? [c] : [] });
      if (c && c.__detached) {
        var html = "";
        try { html = __serEl(c); } catch (e) { html = ""; }
        __push({ type: "insert_html", selector: n.sel, value: html });
        __lsmMarkMounted(c);
      } else if (c && c.__lsm_node) {
        __push({ type: "insert_node", selector: c.__lsm_node.sel, key: n.sel });
      } else if (c && c.nodeType === 3) {
        __lsmPushText(n, c);
      }
      return c;
    },
    removeChild: function (c) {
      var ri = dynKids.indexOf(c);
      if (ri >= 0) dynKids.splice(ri, 1);
      __lsmRec("childList", el, { removedNodes: c ? [c] : [] });
      if (c && c.__lsm_node) __push({ type: "remove_node", selector: c.__lsm_node.sel });
      return c;
    },
    remove: function () {
      var parent = el.parentNode;
      __lsmRec("childList", parent || el, { removedNodes: [el] });
      __push({ type: "remove_node", selector: n.sel });
    },
    insertAdjacentHTML: function (pos, html) {
      var p = String(pos || "beforeend").toLowerCase();
      var h = String(html == null ? "" : html);
      // 本地覆盖层同步记账:插入的 HTML 用伪元素占位,`t.lastChild` 等
      // 立即可读可写(co 辅助函数 `insertAdjacentHTML + lastChild` 模式)。
      var pseudo = __newEl("div");
      pseudo.__pendingHtml = h;
      __lsmRec("childList", el, { addedNodes: [pseudo] });
      if (p === "afterbegin") { dynKids.unshift(pseudo); __push({ type: "insert_html", selector: n.sel, value: h }); }
      else { dynKids.push(pseudo); __push({ type: "append_html", selector: n.sel, value: h }); }
    },
    insertAdjacentElement: function (pos, c) { this.appendChild(c); return c; },
    insertAdjacentText: function (pos, text) {
      this.insertAdjacentHTML(pos, "<span>" + String(text == null ? "" : text) + "</span>");
    },
    // append/prepend:站点常用(缺了直接抛 "not a callable function" 打断整段脚本,
    // 实测 example.com 的 s.js 就卡在 `B.prepend(p)`)。
    append: function () {
      var kids = Array.prototype.slice.call(arguments);
      for (var i = 0; i < kids.length; i++) {
        dynKids.push(kids[i]);
        __lsmInsertChild(el, n.sel, kids[i], false);
      }
    },
    prepend: function () {
      var kids = Array.prototype.slice.call(arguments);
      for (var i = kids.length - 1; i >= 0; i--) {
        dynKids.unshift(kids[i]);
        __lsmInsertChild(el, n.sel, kids[i], true);
      }
    },
    replaceChildren: function () {
      htmlCleared = true;
      dynKids.length = 0;
      __lsmRec("childList", el, {});
      var kids = Array.prototype.slice.call(arguments);
      for (var i = 0; i < kids.length; i++) __lsmInsertChild(el, n.sel, kids[i], false);
    },
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
  // 动态子节点覆盖层:appendChild/insertBefore 的写入对页面脚本**立即可见**
  // (真实浏览器语义)。宿主动作回写是轮间的,若 children 只从快照父索引推导,
  // `container.appendChild(el); container.children[0]` 拿到 undefined,
  // 整条初始化链当场断(实测百度地图 initmap 的 platform = t.children[0])。
  // 元素包装器每次 refresh 重建,覆盖层随之自然重置(彼时宿主 DOM 已应用动作)。
  var dynKids = [];
  var htmlCleared = false;
  // attributes 集合(NamedNodeMap 近似,见 __attrMap 注释):getter 保证
  // 每次 `el.attributes` 都是读当前 attrs 容器的活视图。
  define("attributes", function () { return __attrMap(el, function () { return attrs; }); }, function (v) {});
  define("value", function () {
    return Object.prototype.hasOwnProperty.call(__lsmValues, key) ? __lsmValues[key] : (n.value || "");
  }, function (v) {
    __lsmValues[key] = String(v);
    __lsmRec("attributes", el, { attributeName: "value", attributeOldValue: n.value || "" });
    __push({ type: "set_value", selector: n.sel, value: String(v) });
  });
  define("checked", function () {
    return Object.prototype.hasOwnProperty.call(__lsmChecked, key) ? __lsmChecked[key] : !!n.checked;
  }, function (v) {
    __lsmChecked[key] = !!v;
    __lsmRec("attributes", el, { attributeName: "checked", attributeOldValue: n.checked ? "true" : "false" });
    __push({ type: "set_checked", selector: n.sel, value: v ? "true" : "false" });
  });
  // 真实的父子关系:站点脚本常走 `el.parentNode.insertBefore(...)` 这类路径,
  // 此前 parentNode 恒为 null、children 恒为空,脚本当场抛异常。
  define("parentNode", function () {
    var p = n.parent;
    if (p == null || p < 0 || p >= __lsmNodes.length) return null;
    var pn = __lsmNodes[p];
    return pn ? __mk(pn) : null;
  }, function (v) {});
  define("parentElement", function () { return el.parentNode; }, function (v) {});
  define("children", function () {
    var out = [];
    if (!htmlCleared) {
      for (var i = 0; i < __lsmNodes.length; i++) {
        if (__lsmNodes[i].parent === n.i) out.push(__mk(__lsmNodes[i]));
      }
    }
    for (var d = 0; d < dynKids.length; d++) out.push(dynKids[d]);
    return out;
  }, function (v) {});
  define("childNodes", function () { return el.children; }, function (v) {});
  define("firstChild", function () { return el.children.length ? el.children[0] : null; }, function (v) {});
  define("lastChild", function () {
    var c = el.children;
    return c.length ? c[c.length - 1] : null;
  }, function (v) {});
  define("textContent", function () {
    return Object.prototype.hasOwnProperty.call(__lsmText, key) ? __lsmText[key] : (n.text || "");
  }, function (v) {
    __lsmText[key] = String(v);
    __lsmRec("characterData", el, { oldValue: n.text || "" });
    __push({ type: "set_text", selector: n.sel, value: String(v) });
  });
  define("innerText", function () { return el.textContent; }, function (v) { el.textContent = v; });
  // className 反射到 class 属性:站点普遍 `el.className = "x"` 改样式,
  // 不回传宿主的话序列化 DOM 永远拿不到改后的类名(百度地图节日主题即如此)。
  define("className", function () {
    return Object.prototype.hasOwnProperty.call(attrs, "class") ? String(attrs["class"]) : String(n.class || "");
  }, function (v) {
    __lsmRec("attributes", el, { attributeName: "class", attributeOldValue: __lsmAttrOld(attrs, "class") });
    attrs["class"] = String(v);
    __push({ type: "set_attr", selector: n.sel, key: "class", value: String(v) });
  });
  define("innerHTML", function () {
    if (Object.prototype.hasOwnProperty.call(__lsmText, key)) return __lsmText[key];
    if (n.attrs && n.attrs.outer_html) return n.attrs.outer_html;
    // innerHTML 是**子节点的序列化 HTML**,不是可见文本。
    // 此前回退到 `n.text`(元素的可见文本),于是 `document.body.innerHTML`
    // 只有一个元素的文字(实测百度地图:7 个字符「百度地图 mz」),
    // 站点脚本与 Agent 都会误判「页面是空壳」。
    var inner = "";
    for (var si = 0; si < __lsmNodes.length; si++) {
      if (__lsmNodes[si].parent === n.i) {
        try { inner += __serSnapNode(__lsmNodes[si]); } catch (e) {}
      }
    }
    for (var dk = 0; dk < dynKids.length; dk++) {
      try { inner += __serEl(dynKids[dk]); } catch (e) {}
    }
    return inner;
  }, function (v) {
    var s = String(v);
    __lsmText[key] = s;
    // 本地视角:清空既有子节点(innerHTML=""),覆盖层同步清空。
    htmlCleared = true;
    dynKids.length = 0;
    __lsmRec("childList", el, {});
    __push({ type: "set_html", selector: n.sel, value: s });
  });
  define("outerHTML", function () {
    if (n.attrs && n.attrs.outer_html) return n.attrs.outer_html;
    try { return __serSnapNode(n); } catch (e) { return n.text || ""; }
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
    __lsmRec("attributes", el, { attributeName: "style", attributeOldValue: styleCss });
    styleCss = css;
    __push({ type: "set_attr", selector: n.sel, key: "style", value: css });
    attrs.style = css;
  });
  // ownerDocument 指向 document(document.defaultView → window 在 window 创建后统一回填)。
  el.ownerDocument = document;
  // parentNode/parentElement:按**当前**快照树实时解析(数组下标会随 DOM 变化错位)。
  // Sizzle/jQuery 的上下文查询与 `closest` 依赖父链,缺失时元素作用域查询落空。
  try {
    Object.defineProperty(el, "parentNode", {
      configurable: true,
      enumerable: true,
      get: function () {
        var self = __lsmCurrentIdx(n);
        if (self < 0) return null;
        var p = __lsmNodes[self] ? __lsmNodes[self].parent : -1;
        if (p == null || p < 0 || p >= __lsmNodes.length) return null;
        return __mk(__lsmNodes[p]);
      }
    });
    Object.defineProperty(el, "parentElement", {
      configurable: true,
      enumerable: true,
      get: function () { return this.parentNode; }
    });
    // compareDocumentPosition:DOM 顺序判定(位掩码,仅实现内核用得到的语义)。
    // Sizzle 的 contains/uniqueSort 依赖它,缺失会让上下文查询结果被丢弃。
    el.compareDocumentPosition = function (other) {
      var self = __lsmCurrentIdx(n);
      var oi = (other && other.__lsm_node) ? __lsmCurrentIdx(other.__lsm_node) : -1;
      if (self < 0 || oi < 0) return 0;
      if (__ancestorChain(oi).indexOf(self) >= 0) return 16 | 32; // CONTAINED_BY | FOLLOWING
      if (__ancestorChain(self).indexOf(oi) >= 0) return 10; // CONTAINS | PRECEDING
      return 4; // DISCONNECTED
    };
    // contains:祖先判定。事件委托(站点 `parents.contains(target)`)依赖它,
    // 缺失时点击委托处理器直接抛错(实测百度地图路线按钮点击无反应)。
    el.contains = function (other) {
      if (!other) return false;
      if (other === el) return true;
      var self = __lsmCurrentIdx(n);
      var oi = other.__lsm_node ? __lsmCurrentIdx(other.__lsm_node) : -1;
      if (self < 0 || oi < 0) return false;
      return __ancestorChain(oi).indexOf(self) >= 0;
    };
  } catch (e) {}
  return el;
}

// ---------- URL 解析(供 <a> 反射;GLOBALS_JS 的 URL 不可跨作用域复用) ----------
// 真实浏览器给 <a> 设置 href 后自动反射 protocol/host/pathname/search/hash
// 等解析结果;axios 的 URL 判同模块在装载期就调用 `a.pathname.charAt(0)`,
// 缺失反射 = undefined.charAt 当场 ToObject 崩、整库装不上
// (实测 zhipin bossui.min.js)。
function __lsmParseUrl(u, base) {
  var s = String(u == null ? "" : u);
  if (!/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(s) && base != null) {
    var bm = /^([a-zA-Z][a-zA-Z0-9+.-]*:)\/\/([^\/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/.exec(String(base));
    if (bm) {
      var origin = bm[1] + "//" + bm[2];
      var bpath = bm[3] || "/";
      if (s.charAt(0) === "/") s = origin + s;
      else if (s.charAt(0) === "?") s = origin + bpath + s;
      else if (s.charAt(0) === "#") s = origin + bpath + (bm[4] || "") + s;
      else s = origin + bpath.replace(/[^/]*$/, "") + s;
    }
  }
  var pm = /^[a-zA-Z][a-zA-Z0-9+.-]*:/.exec(s);
  var scheme = pm ? pm[0].toLowerCase() : "";
  var rest = pm ? s.slice(pm[0].length) : s;
  // 授权段前缀 `//`:先剥离再取 host,否则 `//host/path` 的协议斜杠会被
  // 吞进 pathname(实测 a.pathname 反射成 //host/x/y)。
  if (rest.indexOf("//") === 0) rest = rest.slice(2);
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
  return {
    href: s, protocol: scheme, host: host, hostname: hostOnly, port: port,
    origin: (scheme && hostOnly) ? scheme + "//" + hostOnly : "",
    pathname: pathname, search: query, hash: hash
  };
}
function __newEl(tag) {
  var t = String(tag || "div").toLowerCase();
  var el = {
    __detached: true,
    // 动态节点稳定键:快照节点用索引(s<i>),动态节点用自增 uid(d<uid>)。
    __lsmUid: ++__lsmUidSeq,
    __tag: t,
    __attrs: {},
    __kids: [],
    // innerHTML 赋值产生的可查询子树(仅用于 children/getElementsByTagName/
    // querySelector 等读路径;序列化仍用 __pendingHtml 原文保真)。
    __htmlKids: [],
    __text: "",
    nodeType: 1,
    tagName: t.toUpperCase(),
    nodeName: t.toUpperCase(),
    className: "",
    id: "",
    style: __mkStyle(function () { return el.__attrs.style || ""; }, function (css) { el.__attrs.style = css; }),
    dataset: {},
    // 几何属性不用字面量:改由下方 def() 装成 getter,按 `__lsmDynGeom` 实时读取
    // (宿主已把动态子树纳入布局,见 __lsmDynGeo 注释)。
    clientLeft: 0, clientTop: 0,
    children: [],
    childNodes: [],
    setAttribute: function (k, v) {
      __lsmRec("attributes", el, { attributeName: String(k), attributeOldValue: __lsmAttrOld(this.__attrs, String(k)) });
      this.__attrs[String(k)] = String(v); if (k === "id") this.id = String(v); if (k === "class") this.className = String(v);
    },
    setAttributeNS: function (ns, k, v) { this.setAttribute(k, v); },
    getAttribute: function (k) { return Object.prototype.hasOwnProperty.call(this.__attrs, String(k)) ? this.__attrs[String(k)] : null; },
    getAttributeNS: function (ns, k) { return this.getAttribute(k); },
    hasAttribute: function (k) { return Object.prototype.hasOwnProperty.call(this.__attrs, String(k)); },
    hasAttributeNS: function (ns, k) { return this.hasAttribute(k); },
    removeAttribute: function (k) {
      __lsmRec("attributes", el, { attributeName: String(k), attributeOldValue: __lsmAttrOld(this.__attrs, String(k)) });
      delete this.__attrs[String(k)];
    },
    removeAttributeNS: function (ns, k) { this.removeAttribute(k); },
    appendChild: function (c) {
      this.__kids.push(c);
      if (c && c.__tag) c.__parentDyn = el;
      try { __lsmRegisterScriptLoad(c); } catch (e) {}
      __lsmRec("childList", el, { addedNodes: c ? [c] : [] });
      __lsmPushDynChild(el, c, false);
      return c;
    },
    insertBefore: function (c) {
      this.__kids.push(c);
      if (c && c.__tag) c.__parentDyn = el;
      try { __lsmRegisterScriptLoad(c); } catch (e) {}
      __lsmRec("childList", el, { addedNodes: c ? [c] : [] });
      __lsmPushDynChild(el, c, true);
      return c;
    },
    removeChild: function (c) {
      var i = this.__kids.indexOf(c);
      if (i >= 0) this.__kids.splice(i, 1);
      __lsmRec("childList", el, { removedNodes: c ? [c] : [] });
      return c;
    },
    insertAdjacentHTML: function (pos, html) {
      var p = String(pos || "beforeend").toLowerCase();
      var pseudo = __newEl("div");
      pseudo.__pendingHtml = String(html == null ? "" : html);
      if (p === "afterbegin" && this.__kids.length) this.insertBefore(pseudo, this.__kids[0]);
      else this.appendChild(pseudo);
    },
    insertAdjacentElement: function (pos, c) { this.appendChild(c); return c; },
    // 动态节点的 append/prepend:进本地子树,并回写宿主(动态节点以 data-lsm-id 定位)。
    append: function () {
      var kids = Array.prototype.slice.call(arguments);
      for (var i = 0; i < kids.length; i++) {
        this.__kids.push(kids[i]);
        if (kids[i] && kids[i].__tag) kids[i].__parentDyn = el;
        try { __lsmRegisterScriptLoad(kids[i]); } catch (e) {}
        __lsmRec("childList", el, { addedNodes: [kids[i]] });
        __lsmPushDynChild(el, kids[i], false);
      }
    },
    prepend: function () {
      var kids = Array.prototype.slice.call(arguments);
      for (var i = kids.length - 1; i >= 0; i--) {
        this.__kids.unshift(kids[i]);
        if (kids[i] && kids[i].__tag) kids[i].__parentDyn = el;
        try { __lsmRegisterScriptLoad(kids[i]); } catch (e) {}
        __lsmRec("childList", el, { addedNodes: [kids[i]] });
        __lsmPushDynChild(el, kids[i], true);
      }
    },
    replaceChildren: function () {
      el.__kids = [];
      __lsmRec("childList", el, {});
      var kids = Array.prototype.slice.call(arguments);
      for (var i = 0; i < kids.length; i++) {
        this.__kids.push(kids[i]);
        __lsmRec("childList", el, { addedNodes: [kids[i]] });
      }
      __lsmPushDynContent(el);
    },
    remove: function () {
      var p = el.__parentDyn;
      if (p && p.__kids) {
        var i = p.__kids.indexOf(el);
        if (i >= 0) p.__kids.splice(i, 1);
      }
      // 脱离文档的子树不得再进入 document 查询;重新 append 时
      // __lsmPushDynChild 会重新挂回父链并恢复 mounted 标记。
      if (el.__mounted) {
        var key = __lsmDynKey(el);
        if (key) __push({ type: "remove_node", selector: '[data-lsm-id="' + key + '"]' });
      }
      el.__mounted = false;
      el.__parentDyn = null;
      el.__parentStatic = null;
      __lsmRec("childList", p || el, { removedNodes: [el] });
    },
    // 动态节点也要能绑事件:站点「建模板 → cloneNode → 挂载」出来的控件
    // (百度地图路线面板 .route-button)依赖这里的监听,空实现等于按钮点了没反应。
    addEventListener: function (type, fn) {
      if (typeof fn !== "function") return;
      __lsmListeners.push({ node: el, type: String(type), fn: fn });
    },
    removeEventListener: function () {},
    click: function () {
      __fireDyn(el, "click");
      __push({ type: "click", selector: "@dyn:" + __lsmDynKey(el) });
    },
    focus: function () { __fireDyn(el, "focus"); },
    blur: function () { __fireDyn(el, "blur"); },
    // 深拷贝:属性 / innerHTML 原文 / 子树都带走(旧实现返回空元素,
    // 站点「建模板 → cloneNode → 挂载」模式克隆出的控件是空壳)。
    cloneNode: function (deep) {
      var d = __newEl(t);
      for (var k in el.__attrs) {
        if (Object.prototype.hasOwnProperty.call(el.__attrs, k)) d.__attrs[k] = el.__attrs[k];
      }
      d.__pendingHtml = el.__pendingHtml == null ? null : String(el.__pendingHtml);
      d.__text = el.__text;
      d.__htmlKids = (el.__htmlKids || []).slice();
      if (deep !== false) {
        for (var i = 0; i < (el.__kids || []).length; i++) {
          var kid = el.__kids[i];
          d.__kids.push(kid && kid.cloneNode ? kid.cloneNode(true) : kid);
        }
      }
      return d;
    },
    querySelector: function (s) { return __lsmQueryDyn(s, el, false, null); },
    querySelectorAll: function (s) { return __lsmQueryDyn(s, el, false, null); },
    getBoundingClientRect: function () { return __lsmDynRect(el); }
  };
  function def(name, getter, setter) {
    try { Object.defineProperty(el, name, { get: getter, set: setter, configurable: true, enumerable: true }); } catch (e) {}
  }
  // attributes 集合(NamedNodeMap 近似,见 __attrMap 注释)。
  def("attributes", function () { return __attrMap(el, function () { return el.__attrs; }); });
  // childNodes/children 活视图:innerHTML 解析出的 __htmlKids 与动态 __kids
  // 都要反映(jQuery createHTMLDocument 探测即 body.innerHTML 两 form 后读
  // childNodes.length;静态空数组恒 0,探测恒 false)。
  def("childNodes", function () { return (el.__htmlKids || []).concat(el.__kids || []); });
  def("children", function () { return (el.__htmlKids || []).concat(el.__kids || []).filter(function (k) { return k && k.nodeType === 1; }); });
  // 几何 getter:宿主已把动态子树重新解析进 DOM 并参与布局,rect 随每轮快照回填。
  // 站点读到的尺寸因此与截图里画出来的盒子一致(地图才能按真实容器尺寸装载)。
  def("clientWidth", function () { return __lsmDynGeo(el).w; });
  def("clientHeight", function () { return __lsmDynGeo(el).h; });
  def("offsetWidth", function () { return __lsmDynGeo(el).w; });
  def("offsetHeight", function () { return __lsmDynGeo(el).h; });
  def("scrollWidth", function () { return __lsmDynGeo(el).w; });
  def("scrollHeight", function () { return __lsmDynGeo(el).h; });
  def("offsetLeft", function () { return __lsmDynGeo(el).x; });
  def("offsetTop", function () { return __lsmDynGeo(el).y; });
  // 登记到动态节点表:序列化时据此发 `data-lsm-id`,事件派发也按它反查。
  var __key = __lsmDynKey(el);
  if (__key) __lsmDynById[__key] = el;
  // 记录动态父节点(remove() 与事件冒泡链需要)。
  el.__parentDyn = null;
  // contains:动态子树的祖先判定(事件委托按 target 找祖先)。
  el.contains = function (other) {
    if (!other) return false;
    if (other === el) return true;
    var cur = el;
    var guard = 0;
    while (cur && guard++ < 64) {
      if (cur === other) return true;
      cur = cur.__parentDyn || null;
    }
    return false;
  };
  // 反射属性:站点用 `document.createElement("script").src = url` 动态插脚本,
  // 属性必须真正落到 __attrs 上,否则序列化出来的标签是空的,宿主取不到脚本。
  ["src", "href", "type", "rel", "media", "title", "alt", "charset", "name", "value",
   "id", "class", "content", "async", "defer"].forEach(function (p) {
    def(p, function () { return el.__attrs[p] == null ? "" : String(el.__attrs[p]); },
      function (v) {
        el.__attrs[p] = String(v);
        // 「先 appendChild 再设 src」的装载模式(webpack chunk 加载器即如此):
        // 注册时无 src 是空操作,src 落定这一刻触发同步装载。
        if (p === "src" && t === "script") {
          try { __lsmLoadDynScript(el, __lsmScriptKey(el.__attrs.src || "")); } catch (e) {}
        }
      });
  });
  // <a> 的 URL 反射:href 赋值后按当前页面 location 解析,
  // protocol/host/hostname/port/pathname/search/hash/origin 可读
  // (见 __lsmParseUrl 注释;axios 判同模块在装载期就依赖 pathname)。
  if (t === "a") {
    el.__urlParts = null;
    def("href",
      function () { return el.__attrs.href == null ? "" : String(el.__attrs.href); },
      function (v) {
        el.__attrs.href = String(v);
        el.__urlParts = __lsmParseUrl(el.__attrs.href, (location && location.href) || null);
      });
    ["protocol", "host", "hostname", "port", "pathname", "search", "hash", "origin"].forEach(function (p) {
      def(p, function () {
        if (!el.__urlParts) {
          el.__urlParts = __lsmParseUrl(el.__attrs.href || "", (location && location.href) || null);
        }
        return el.__urlParts ? el.__urlParts[p] : "";
      });
    });
  }
  // <img> 的 src 赋值:BMap 瓦片装载器依赖 onload 推进(`new Image` /
  // `createElement("img")` → 设 src → onload 才把瓦片写进 mapPane 并触发
  // 地图 load 事件)。纯文本内核不做像素解码,这里按异步 load 语义放行:
  // 默认 256×256(地图瓦片标准尺寸),真实像素由宿主图片管线随后取回渲染。
  if (t === "img") {
    el.onload = null;
    el.onerror = null;
    el.complete = false;
    el.naturalWidth = 0;
    el.naturalHeight = 0;
    el.width = 0;
    el.height = 0;
    el.crossOrigin = null;
    def("src",
      function () { return el.__attrs.src == null ? "" : String(el.__attrs.src); },
      function (v) {
        var url = String(v == null ? "" : v);
        el.__attrs.src = url;
        if (!url) return;
        setTimeout(function () {
          // src 在加载前被改写:旧请求作废(真实浏览器同样不会触发旧 onload)。
          if (String(el.__attrs.src || "") !== url) return;
          el.complete = true;
          if (!el.naturalWidth) el.naturalWidth = 256;
          if (!el.naturalHeight) el.naturalHeight = 256;
          if (!el.width) el.width = el.naturalWidth;
          if (!el.height) el.height = el.naturalHeight;
          try {
            if (typeof el.onload === "function") el.onload.call(el, { type: "load", target: el });
          } catch (e) { __cerror("img onload: " + String((e && e.stack) || e)); }
          // 监听器通道:真实浏览器里 load 事件既走 onload 属性,也走
          // addEventListener('load') 注册的监听器。旧实现只走前者,站点
          // 用监听器时事件永不触发 → 瓦片永不「就绪」→ 停在入场前状态
          // (百度地图瓦片停在 opacity:0,地图区域整片空白)。
          try { __fireDyn(el, "load"); } catch (e2) { __cerror("img load 事件: " + String((e2 && e2.stack) || e2)); }
        }, 0);
      });
  }
  // className 写通 class 属性:游离元素序列化时才能带上类名
  // (站点普遍 `el.className = "x"`;此前只有 setAttribute 会落到 __attrs)。
  def("className", function () {
    return el.__attrs["class"] == null ? "" : String(el.__attrs["class"]);
  }, function (v) {
    el.__attrs["class"] = String(v);
  });
  def("innerHTML", function () {
    if (el.__pendingHtml != null) return el.__pendingHtml;
    return __serEl({ __tag: t, __attrs: {}, __kids: el.__kids, __text: "" });
  }, function (v) {
    el.__kids = [];
    el.__text = "";
    // 元素未入树:宿主端 @created set_html 无处落地,内容存 __pendingHtml,
    // 挂载时随序列化原样带到宿主(见 __serEl)。
    el.__pendingHtml = String(v);
    // 同时解析出可查询子树:tangram 特性探测即 createElement→innerHTML→
    // getElementsByTagName('input')[0]→input.checked,缺失子树会让 input 为
    // undefined 并整层中断(实测百度地图白屏)。
    try { el.__htmlKids = __parseHtml(el.__pendingHtml); } catch (e) { el.__htmlKids = []; }
    __lsmRec("childList", el, {});
    __push({ type: "set_html", selector: "@created:" + t + ":" + el.__attrs.id, value: String(v) });
  });
  def("textContent", function () { return el.__pendingHtml != null ? el.__pendingHtml : el.__text; },
    function (v) {
      __lsmRec("characterData", el, { oldValue: el.__text || "" });
      el.__text = String(v); el.__kids = []; el.__pendingHtml = null;
    });
  // 子树查找:detached 元素树上按标签/类收集(消息中心等 widget 对
  // createElement 建出的子树直接 getElementsByTagName)。
  // 子树遍历:innerHTML 解析出的 __htmlKids + 动态 __kids 都要覆盖。
  function __walkKids(node, fn) {
    var groups = [node.__htmlKids || [], node.__kids || []];
    for (var g = 0; g < groups.length; g++) {
      var ks = groups[g];
      for (var i = 0; i < ks.length; i++) {
        fn(ks[i]);
        __walkKids(ks[i], fn);
      }
    }
  }
  el.getElementsByTagName = function (t) {
    var tag = String(t).toLowerCase();
    // 活集合(见 __lsmLive):动态子树改动后立刻可见。
    return __lsmLive(function () {
      var out = [];
      __walkKids(el, function (k) {
        var kt = String((k.__tag || k.tagName || "")).toLowerCase();
        if (tag === "*" || kt === tag) out.push(k);
      });
      return out;
    });
  };
  el.getElementsByClassName = function (c) {
    var cls = String(c);
    return __lsmLive(function () {
      var out = [];
      __walkKids(el, function (k) {
        var kc = String((k.className != null ? k.className : (k.__attrs && k.__attrs["class"]) || ""));
        if (cls === "*" || kc.split(/\s+/).indexOf(cls) >= 0) out.push(k);
      });
      return out;
    });
  };
  def("innerText", function () { return el.__text; }, function (v) { el.__text = String(v); el.__kids = []; });
  def("value", function () { return el.__attrs.value || ""; }, function (v) { el.__attrs.value = String(v); });
  // canvas:2D 上下文以「可调用的空实现」提供,让重度依赖 canvas 的页面
  // (百度地图矢量渲染)完成初始化与事件绑定;像素内容暂不绘制。
  if (t === "canvas") {
    el.width = 300;
    el.height = 150;
    el.getContext = function (kind) {
      var k = String(kind || "").toLowerCase();
      if (k.indexOf("2d") !== 0) {
        // WebGL / experimental-webgl:内核没有 GPU 后端,一律给 null(与真实
        // 无 WebGL 环境一致)。但**记一笔**:站点据此判定能力并(如百度地图)
        // 直接放弃整个地图渲染,页面只剩空壳。记下来如实告知调用方,免得
        // Agent 反复重试一个内核根本画不出来的区域。
        if (k.indexOf("webgl") === 0) window.__lsmWebGLAsked = (window.__lsmWebGLAsked || 0) + 1;
        return null;
      }
      if (!el.__ctx2d) el.__ctx2d = __mkCanvas2d(el);
      return el.__ctx2d;
    };
    el.toDataURL = function () { return "data:image/png;base64,"; };
    el.getBoundingClientRect = function () {
      return { x: 0, y: 0, top: 0, left: 0, right: el.width, bottom: el.height, width: el.width, height: el.height };
    };
  }
  // children 反射 __kids:createElement+appendChild 建出的子树立即被父节点可见
  // (百度地图 render() 容器 → platform = t.children[0] 链路依赖此语义)。
  // children 只含元素节点(浏览器语义);childNodes 含文本节点。
  def("children", function () {
    return (el.__htmlKids || []).concat(el.__kids).filter(function (k) { return k && k.nodeType === 1; });
  }, function (v) {});
  def("childNodes", function () { return (el.__htmlKids || []).concat(el.__kids); }, function (v) {});
  def("firstChild", function () {
    var all = (el.__htmlKids || []).concat(el.__kids);
    return all.length ? all[0] : null;
  }, function (v) {});
  def("lastChild", function () {
    var all = (el.__htmlKids || []).concat(el.__kids);
    return all.length ? all[all.length - 1] : null;
  }, function (v) {});
  el.ownerDocument = document;
  // 属性写拦截:挂载后的属性变更(百度地图瓦片淡入 opacity 0→1 等)
  // 必须回写宿主 DOM,否则宿主永远停在挂载时的属性值。
  el.__attrs = __mkDynAttrs(el);
  return el;
}

// canvas 2D 上下文空实现(初始化兼容层,不产生像素)。
function __mkCanvas2d(canvas) {
  var noop = function () {};
  var ctx = {
    canvas: canvas,
    fillStyle: "#000000", strokeStyle: "#000000", lineWidth: 1, lineCap: "butt", lineJoin: "miter",
    miterLimit: 10, font: "10px sans-serif", textAlign: "start", textBaseline: "alphabetic",
    globalAlpha: 1, globalCompositeOperation: "source-over", imageSmoothingEnabled: true,
    shadowBlur: 0, shadowColor: "rgba(0,0,0,0)", shadowOffsetX: 0, shadowOffsetY: 0,
    save: noop, restore: noop, beginPath: noop, closePath: noop, moveTo: noop, lineTo: noop,
    quadraticCurveTo: noop, bezierCurveTo: noop, arc: noop, arcTo: noop, ellipse: noop, rect: noop,
    fill: noop, stroke: noop, clip: noop, fillRect: noop, strokeRect: noop, clearRect: noop,
    translate: noop, rotate: noop, scale: noop, transform: noop, setTransform: noop, resetTransform: noop,
    drawImage: noop, putImageData: noop, setLineDash: noop, getLineDash: function () { return []; },
    fillText: noop, strokeText: noop,
    measureText: function (t) {
      var size = parseFloat(ctx.font) || 10;
      return { width: String(t == null ? "" : t).length * size * 0.55 };
    },
    createLinearGradient: function () { return { addColorStop: noop }; },
    createRadialGradient: function () { return { addColorStop: noop }; },
    createPattern: function () { return {}; },
    createImageData: function (w, h) {
      w = Math.max(0, w | 0); h = Math.max(0, h | 0);
      var data;
      try { data = new Uint8ClampedArray(w * h * 4); } catch (e) { data = []; }
      return { width: w, height: h, data: data };
    },
    getImageData: function (x, y, w, h) {
      w = Math.max(0, w | 0); h = Math.max(0, h | 0);
      var data;
      try { data = new Uint8ClampedArray(w * h * 4); } catch (e) { data = []; }
      return { width: w, height: h, data: data };
    },
    isPointInPath: function () { return false; },
    isPointInStroke: function () { return false; },
    getContextAttributes: function () { return {}; }
  };
  return ctx;
}

// ---------- detached innerHTML 轻量解析 ----------
// 只服务于「建好子树后按标签/类查询」这类读路径(tangram/jQuery 特性探测、
// widget 模板拼装)。不做实体解码/命名空间/错误恢复,序列化仍走 __pendingHtml 原文。
var __HTML_VOID = { area: 1, base: 1, br: 1, col: 1, embed: 1, hr: 1, img: 1, input: 1, link: 1, meta: 1, param: 1, source: 1, track: 1, wbr: 1 };
function __mkTextNode(s) { return { nodeType: 3, textContent: String(s), data: String(s) }; }
function __parseHtml(html) {
  var s = String(html == null ? "" : html);
  var root = [];
  var stack = [{ kids: root, tag: "" }];
  var i = 0, n = s.length;
  function top() { return stack[stack.length - 1]; }
  while (i < n) {
    if (s.charAt(i) === "<") {
      if (s.substr(i, 4) === "<!--") {
        var ce = s.indexOf("-->", i + 4);
        i = ce < 0 ? n : ce + 3;
        continue;
      }
      if (s.charAt(i + 1) === "!" || s.charAt(i + 1) === "?") {
        var de = s.indexOf(">", i + 2);
        i = de < 0 ? n : de + 1;
        continue;
      }
      if (s.charAt(i + 1) === "/") {
        var gt = s.indexOf(">", i + 2);
        if (gt < 0) break;
        var closeTag = s.slice(i + 2, gt).trim().toLowerCase();
        for (var k = stack.length - 1; k > 0; k--) {
          if (stack[k].tag === closeTag) { stack.length = k; break; }
        }
        i = gt + 1;
        continue;
      }
      var j = i + 1, quote = 0, end = -1;
      for (; j < n; j++) {
        var ch = s.charAt(j);
        if (quote) { if (ch === quote) quote = 0; continue; }
        if (ch === '"' || ch === "'") { quote = ch; continue; }
        if (ch === ">") { end = j; break; }
      }
      if (end < 0) break;
      var raw = s.slice(i + 1, end);
      var selfClose = /\/\s*$/.test(raw);
      if (selfClose) raw = raw.replace(/\/\s*$/, "");
      var sp = raw.search(/[\s\/]/);
      var tag = (sp < 0 ? raw : raw.slice(0, sp)).toLowerCase();
      var attrStr = sp < 0 ? "" : raw.slice(sp);
      var el = __newEl(tag);
      var ai = 0, an = attrStr.length;
      while (ai < an) {
        while (ai < an && /\s/.test(attrStr.charAt(ai))) ai++;
        if (ai >= an) break;
        var astart = ai;
        while (ai < an && !/[\s=]/.test(attrStr.charAt(ai))) ai++;
        var name = attrStr.slice(astart, ai);
        if (!name) { ai++; continue; }
        while (ai < an && /\s/.test(attrStr.charAt(ai))) ai++;
        var val = "";
        if (attrStr.charAt(ai) === "=") {
          ai++;
          while (ai < an && /\s/.test(attrStr.charAt(ai))) ai++;
          var q = attrStr.charAt(ai);
          if (q === '"' || q === "'") {
            var e = attrStr.indexOf(q, ai + 1);
            if (e < 0) e = an;
            val = attrStr.slice(ai + 1, e);
            ai = e + 1;
          } else {
            var vs = ai;
            while (ai < an && !/\s/.test(attrStr.charAt(ai))) ai++;
            val = attrStr.slice(vs, ai);
          }
        }
        el.__attrs[name.toLowerCase()] = val;
        if (name === "id") el.id = val;
        if (name === "class") el.className = val;
      }
      top().kids.push(el);
      if (!selfClose && !__HTML_VOID[tag]) {
        if (tag === "script" || tag === "style" || tag === "textarea" || tag === "title") {
          var lower = s.toLowerCase();
          var ci = lower.indexOf("</" + tag, end + 1);
          var contentEnd = ci < 0 ? n : ci;
          var content = s.slice(end + 1, contentEnd);
          if (content) el.__kids.push(__mkTextNode(content));
          var cgt = ci < 0 ? -1 : s.indexOf(">", ci);
          i = cgt < 0 ? n : cgt + 1;
          continue;
        }
        stack.push({ kids: el.__kids, tag: tag });
      }
      i = end + 1;
      continue;
    }
    var nxt = s.indexOf("<", i);
    if (nxt < 0) nxt = n;
    var txt = s.slice(i, nxt);
    if (txt) top().kids.push(__mkTextNode(txt));
    i = nxt;
  }
  return root;
}

// ---------- document ----------
var document = {
  nodeType: 9,
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
  querySelector: __lsmQueryOneWithDyn,
  querySelectorAll: __lsmQueryWithDyn,
  getElementById: function (id) { return __lsmQueryOneWithDyn("#" + id); },
  getElementsByTagName: function (t) {
    var tag = String(t);
    // 活集合(见 __lsmLive)。
    return __lsmLive(function () {
      if (tag === "*") return __lsmQuery("*");
      return __lsmQuery(tag);
    });
  },
  getElementsByClassName: function (c) {
    var cls = String(c);
    return __lsmLive(function () { return __lsmQuery("." + cls); });
  },
  getElementsByName: function (n) {
    var nm = String(n);
    return __lsmLive(function () { return __lsmQuery("[name=" + nm + "]"); });
  },
  // document.contains:祖先判定(事件委托/工具库常用)。
  contains: function (other) {
    try {
      var de = document.documentElement;
      return de ? !!de.contains(other) : false;
    } catch (e) { return false; }
  },
  createElement: function (tag) { return __newEl(tag); },
  // createEvent:老式事件构造入口(Vue 1.0 的 performance.now 校准、站点
  // 合成键盘/自定义事件)。缺失时 `document.createEvent("Event")` 报
  // "not a callable function" 直接打断库装载(实测 zhipin vue-core-v1.0.0)。
  createEvent: function (type) {
    var t = String(type || "Event");
    return {
      type: "", eventType: t, bubbles: false, cancelable: false,
      timeStamp: __lsmNow, target: null, currentTarget: null,
      defaultPrevented: false, propagationStopped: false,
      initEvent: function (ty, b, c) { this.type = String(ty); this.bubbles = !!b; this.cancelable = !!c; },
      preventDefault: function () { this.defaultPrevented = true; },
      stopPropagation: function () { this.propagationStopped = true; },
      stopImmediatePropagation: function () { this.propagationStopped = true; }
    };
  },
  // createElementNS:SVG/图标站点(图表、地图图例)用 NS 建元素;内核暂不渲染 SVG,
  // 但「建出来、能挂树、不崩」即可让面板/widget 完成初始化与挂载。
  createElementNS: function (ns, tag) { return __newEl(tag); },
  createSVGElement: function (tag) { return __newEl(tag); },
  createTextNode: function (t) { return { nodeType: 3, textContent: String(t), data: String(t) }; },
  // createComment:Vue 1.x 的 VDOM 挂载路径用注释节点做 v-if 占位
  // (`createComment('v-if')`),缺失时挂载链当场 "not a callable function"
  // (实测 zhipin app 挂载)。
  createComment: function (t) {
    return { nodeType: 8, nodeName: "#comment", textContent: String(t == null ? "" : t), data: String(t == null ? "" : t), nodeValue: String(t == null ? "" : t) };
  },
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
  queryCommandSupported: function () { return false; },
  // elementFromPoint:坐标命中测试。站点 APM/安全 SDK(patas 空白页采样、
  // ka 指纹采集)在定时器里高频调用,缺失时回调整段抛错,SDK 状态机停摆、
  // 后续依赖它的数据请求(joblist 等)永不发出(实测 zhipin)。
  // 实现:遍历快照节点,取包含该点的**最小**盒子(≈最深元素);无命中返回
  // documentElement(与真实浏览器视口内行为一致)。
  elementFromPoint: function (x, y) {
    var px = Number(x), py = Number(y);
    if (!isFinite(px) || !isFinite(py)) return null;
    var best = -1, bestArea = Infinity;
    for (var i = 0; i < __lsmNodes.length; i++) {
      var nd = __lsmNodes[i];
      var r = nd && nd.rect;
      if (!r || typeof r.length !== "number" || r.length < 4) continue;
      var rx = Number(r[0]) || 0, ry = Number(r[1]) || 0;
      var rw = Number(r[2]) || 0, rh = Number(r[3]) || 0;
      if (rw <= 0 || rh <= 0) continue;
      if (px >= rx && px <= rx + rw && py >= ry && py <= ry + rh) {
        var area = rw * rh;
        if (area <= bestArea) { bestArea = area; best = i; }
      }
    }
    if (best >= 0) return __mk(__lsmNodes[best]);
    return document.documentElement || null;
  },
  // implementation:jQuery/Zepto 的 parseHTML 沙箱探测入口
  // (`implementation.createHTMLDocument("")` → body.innerHTML 两个 form →
  // childNodes.length===2)。此前整个属性缺失,探测链当场 ToObject 崩掉
  // (实测 zhipin jQuery 1.12.2 的 createHTMLDocument 支持性检测)。
  implementation: {
    hasFeature: function () { return true; },
    createHTMLDocument: function (title) {
      var body = __newEl("body");
      var doc = {
        nodeType: 9,
        readyState: "complete",
        title: String(title == null ? "" : title),
        body: body,
        documentElement: __newEl("html"),
        createElement: function (tag) { return __newEl(tag); },
        createElementNS: function (ns, tag) { return __newEl(tag); },
        createTextNode: function (t) { return { nodeType: 3, textContent: String(t), data: String(t) }; },
        createDocumentFragment: function () {
          return { __detached: true, __tag: "div", __attrs: {}, __kids: [], __text: "", nodeType: 11, appendChild: function (c) { this.__kids.push(c); return c; } };
        },
        querySelector: function () { return null; },
        querySelectorAll: function () { return []; },
        getElementsByTagName: function (t) { return body.getElementsByTagName(t); }
      };
      body.ownerDocument = doc;
      return doc;
    }
  }
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
  document.forms = __lsmQuery("form");
  document.links = __lsmQuery("a[href]");
  document.images = __lsmQuery("img");
  document.scripts = __lsmQuery("script");
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
// 定时器注册来源(脚本 URL @ 最后语句标记):回调抛错时据此定位注册点。
function __lsmTimerReg() {
  try {
    return String(globalThis.__lsmCurrentScript || "?") + " @ " + String(__lsmLastStmt || "");
  } catch (e) { return ""; }
}
function setTimeout(fn, delay) {
  var id = ++__lsmTimerSeq;
  if (typeof fn === "function") {
    var args = Array.prototype.slice.call(arguments, 2);
    __lsmTimers.push({ id: id, fn: fn, args: args, next: __lsmNow + __lsmDelay(delay), interval: __lsmDelay(delay), repeat: false, reg: __lsmTimerReg() });
  }
  return id;
}
function setInterval(fn, delay) {
  var id = ++__lsmTimerSeq;
  if (typeof fn === "function") {
    var args = Array.prototype.slice.call(arguments, 2);
    var d = Math.max(16, __lsmDelay(delay));
    __lsmTimers.push({ id: id, fn: fn, args: args, next: __lsmNow + d, interval: d, repeat: true, reg: __lsmTimerReg() });
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
      try { tm.fn.apply(null, tm.args || []); } catch (e) { __cerror("timer[" + String(tm.reg || "") + "]: " + String((e && e.stack) || e)); }
    }
    if (ran >= 64) break;
  }
  // 定时器批次后派发观察者:rAF/定时器驱动的 SPA 靠 MutationObserver 推进状态。
  __lsmFlushObservers();
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
  // console.assert 只记录失败、绝不抛错;站点初始化里的 UUID/配置自检
  // 若因方法缺失抛 TypeError,会让整个 React 挂载链静默中断。
  assert: function (condition) {
    if (!condition) {
      var detail = Array.prototype.slice.call(arguments, 1).map(function (x) { return __str(x); }).join(" ");
      __clog("error", "Assertion failed" + (detail ? ": " + detail : ""));
    }
  },
  dir: function () {},
  table: function () {},
  time: function () {},
  timeEnd: function () {}
};
function __str(x) {
  if (x === null) return "null";
  if (x === undefined) return "undefined";
  if (typeof x === "object") {
    // Error 的 name/message/stack 通常不可枚举，JSON.stringify(new Error(...))
    // 只会得到 "{}"。鸭子类型同时覆盖跨 realm 错误与站点包装的错误对象。
    try {
      if (x instanceof Error || ((x.name || x.message) && x.stack)) return __lsmErrText(x);
    } catch (e) {}
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
  this.responseType = ""; this.responseURL = "";
  this.onload = null; this.onerror = null; this.onreadystatechange = null;
  this._url = ""; this._method = "GET"; this._headers = {}; this._listeners = {};
  this._respHeaders = {};
}
// 真实浏览器派发回调时一定带 Event 实参;此前裸调用导致站点回调里
// 读 `e.target` / 对事件对象做 ToObject 时抛 "cannot convert 'null' or
// 'undefined' to object"(实测百度地图 `xhr rs` 报错,数据 boot 链中断)。
function __lsmXhrEvent(type, xhr) {
  return { type: String(type), target: xhr, currentTarget: xhr, timeStamp: Date.now(),
           bubbles: false, cancelable: false, defaultPrevented: false,
           preventDefault: function () { this.defaultPrevented = true; },
           stopPropagation: function () {} };
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
  for (var i = 0; i < list.length; i++) { try { list[i].call(this, __lsmXhrEvent(name, this)); } catch (e) { __cerror("xhr " + name + ": " + String(e)); } }
};
XMLHttpRequest.prototype.__rs = function () {
  if (this.onreadystatechange) { try { this.onreadystatechange.call(this, __lsmXhrEvent("readystatechange", this)); } catch (e) { __cerror("xhr rs: " + String(e)); } }
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
    if (this.onerror) { try { this.onerror.call(this, __lsmXhrEvent("error", this)); } catch (e4) {} }
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
  // 响应头/最终 URL 回填:`getResponseHeader` 此前永远拿到 null(_respHeaders 从不赋值)。
  this._respHeaders = r.headers || {};
  this.responseURL = String(r.url || this._url);
  if (this.responseType === "json") {
    try { this.response = this.responseText === "" ? null : JSON.parse(this.responseText); }
    catch (e6) { this.response = null; }
  }
  this.__rs();
  if (this.onload) { try { this.onload.call(this, __lsmXhrEvent("load", this)); } catch (e5) { __cerror("xhr load: " + String(e5)); } }
  this.__fire("load");
};
XMLHttpRequest.prototype.abort = function () {};
XMLHttpRequest.prototype.getResponseHeader = function (k) {
  return (this._respHeaders && this._respHeaders[String(k).toLowerCase()]) || null;
};
XMLHttpRequest.prototype.getAllResponseHeaders = function () {
  var out = [];
  var h = this._respHeaders || {};
  for (var k in h) { out.push(k + ": " + h[k]); }
  return out.join("\r\n");
};

// ---------- matchMedia(视口/方向/配色子集) ----------
function __cssLen(v) {
  v = String(v || "").trim().toLowerCase();
  var n = parseFloat(v);
  if (isNaN(n)) return null;
  if (v.indexOf("rem") >= 0 || v.indexOf("em") >= 0) return n * 16;
  if (v.indexOf("pt") >= 0) return n * 4 / 3;
  return n;
}
function __mediaTerm(term) {
  term = String(term || "").trim().toLowerCase();
  if (!term) return true;
  if (term.charAt(0) === "(" && term.charAt(term.length - 1) === ")") {
    var inner = term.slice(1, -1);
    var colon = inner.indexOf(":");
    if (colon < 0) return false;
    var name = inner.slice(0, colon).trim();
    var value = inner.slice(colon + 1).trim();
    var w = Number(window.innerWidth) || 1280;
    var h = Number(window.innerHeight) || 720;
    if (name === "min-width") { var a = __cssLen(value); return a !== null && w >= a; }
    if (name === "max-width") { var b = __cssLen(value); return b !== null && w <= b; }
    if (name === "min-height") { var c = __cssLen(value); return c !== null && h >= c; }
    if (name === "max-height") { var d = __cssLen(value); return d !== null && h <= d; }
    if (name === "orientation") return value === "landscape" ? w >= h : (value === "portrait" ? w < h : false);
    if (name === "prefers-color-scheme") return value === "dark" ? !!window.__lsmPrefersDark : (value === "light" ? !window.__lsmPrefersDark : false);
    return false;
  }
  if (term === "all" || term === "screen") return true;
  return false;
}
function __evalMedia(query) {
  var parts = String(query || "").split(",");
  for (var i = 0; i < parts.length; i++) {
    var raw = parts[i].trim();
    if (!raw) continue;
    var neg = false;
    if (/^not\s+/i.test(raw)) { neg = true; raw = raw.replace(/^not\s+/i, ""); }
    raw = raw.replace(/^only\s+/i, "");
    var terms = raw.split(/\s+and\s+/i);
    var ok = true;
    for (var j = 0; j < terms.length; j++) {
      if (!__mediaTerm(terms[j])) { ok = false; break; }
    }
    if (neg) ok = !ok;
    if (ok) return true;
  }
  return false;
}

// ---------- window ----------
// window 必须是"全局对象的门面":站点脚本普遍用 `window.foo = fn` 定义全局函数
// (百度地图的 `alog` 即是如此)。若 window 只是普通对象,`window.alog` 不会成为
// 全局标识符,裸 `alog` 调用直接 ReferenceError,页面初始化中断、UI 永远建不出来。
var __lsmWin = {
  document: document, location: location, navigator: navigator, screen: screen, history: history,
  localStorage: localStorage, sessionStorage: sessionStorage, console: console,
  innerWidth: 1280, innerHeight: 720, outerWidth: 1280, outerHeight: 720,
  scrollX: 0, scrollY: 0, pageXOffset: 0, pageYOffset: 0,
  devicePixelRatio: 1, name: "", closed: false, isSecureContext: true,
  // 遗留全局:站点脚本直接写裸 `status`(browser window.status 恒存在)。
  status: "", defaultStatus: "",
  getComputedStyle: function (el) {
    var css = {};
    try {
      var n = el && el.__lsm_node;
      if (n && n.css) css = n.css;
      else {
        var inline = String((el && el.style && el.style.cssText) || "");
        var parts = inline.split(";");
        for (var i = 0; i < parts.length; i++) {
          var p = parts[i];
          var c = p.indexOf(":");
          if (c > 0) css[p.slice(0, c).trim().toLowerCase()] = p.slice(c + 1).trim();
        }
      }
    } catch (e) {}
    var out = {};
    for (var k in css) {
      if (!Object.prototype.hasOwnProperty.call(css, k)) continue;
      out[k] = css[k];
      // 兼容 CSSStyleDeclaration 的 camelCase 属性访问。
      out[k.replace(/-([a-z])/g, function (_, c) { return c.toUpperCase(); })] = css[k];
    }
    out.getPropertyValue = function (p) {
      var key = String(p).toLowerCase();
      return Object.prototype.hasOwnProperty.call(css, key) ? css[key] : "";
    };
    return out;
  },
  matchMedia: function (q) {
    var query = String(q || "");
    return {
      matches: __evalMedia(query),
      media: query,
      onchange: null,
      addListener: function () {}, removeListener: function () {},
      addEventListener: function () {}, removeEventListener: function () {},
      dispatchEvent: function () { return false; }
    };
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
var window = __lsmWin;
if (typeof Proxy === "function") {
  window = new Proxy(__lsmWin, {
    get: function (t, k) {
      if (k in t) return t[k];
      if (typeof globalThis !== "undefined" && k in globalThis) return globalThis[k];
      return undefined;
    },
    // has 陷阱:`"onsubmit" in window` 在真实浏览器恒为 true(window 有一整族
    // on* 事件处理器占位属性),站点库拿它做特性检测。缺此陷阱时默认转发到
    // 目标对象,`on*` 名查不到返回 false → jQuery support 误判走 IE<9 兜底
    // 分支 → `div.attributes[name].expando` 链(实测 zhipin jQuery 1.12.2)。
    has: function (t, k) {
      if (k in t) return true;
      if (typeof globalThis !== "undefined" && k in globalThis) return true;
      return typeof k === "string" ? /^on[a-z][a-z0-9]*$/.test(k) : false;
    },
    set: function (t, k, v) {
      // 目标若是「非可配置的只读访问器」(站点反爬脚本的典型手法:
      // `Object.defineProperty(window,'eval',{get:...,configurable:false})`
      // 用来侦测页面是否调用 eval),浏览器语义是**静默失败**(sloppy)/
      // 抛 TypeError(strict),而 Proxy [[Set]] 的后置校验会直接抛
      // "Proxy trap set unexpected accessor descriptor" —— 一次赋值就把
      // 整段站点初始化打断(实测百度地图反爬脚本遮蔽 window.eval)。
      // 这里返回 false:让赋值退化为浏览器的「静默失败」语义。
      try {
        var d = Object.getOwnPropertyDescriptor(t, k);
        if (d && d.configurable === false && !("value" in d) && !d.set) {
          return false;
        }
      } catch (e) {}
      try { t[k] = v; } catch (e) {}
      // 同步为全局标识符:`window.foo = fn` 之后裸 `foo` 必须可见。
      try {
        if (typeof globalThis !== "undefined" && !(k in globalThis)) globalThis[k] = v;
      } catch (e) {}
      return true;
    }
  });
}
var getComputedStyle = function (el) { return window.getComputedStyle(el); };
var matchMedia = function (q) { return window.matchMedia(q); };
window.self = window; window.top = window; window.parent = window; window.frames = window;
window.window = window;
// document.defaultView → window:标准取样式路径 `el.ownerDocument.defaultView
// .getComputedStyle(el, null)` 的关键一环(IE 的 currentStyle 分支之外的主流路径)。
document.defaultView = window;
try {
  Object.defineProperty(window, "location", {
    get: function () { return location; },
    set: function (v) { __push({ type: "navigate", url: String(v) }); },
    configurable: true
  });
} catch (e) {}
// 把 window 的能力同步到全局对象(裸 `performance` / `innerWidth` 等全局标识符)。
(function () {
  if (typeof globalThis === "undefined") return;
  var keys = Object.getOwnPropertyNames(__lsmWin);
  for (var i = 0; i < keys.length; i++) {
    var k = keys[i];
    if (k === "window" || k === "self" || k === "top" || k === "parent" || k === "frames") continue;
    try {
      if (!(k in globalThis)) {
        Object.defineProperty(globalThis, k, Object.getOwnPropertyDescriptor(__lsmWin, k));
      }
    } catch (e) {}
  }
})();

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
// 动态子树冒泡链:目标快照节点若被动态对象收养,把它的动态父链一并纳入,
// 这样「注册在动态节点上的监听」在宿主点击命中其子树时也能跑到。
function __dynChainOf(node) {
  var out = [];
  var dk = node && node.attrs ? node.attrs["data-lsm-id"] : null;
  var cur = dk ? __lsmDynById[dk] : null;
  var guard = 0;
  while (cur && guard++ < 64) {
    out.push(cur);
    cur = cur.__parentDyn || null;
  }
  return out;
}
function __fire(sel, type) {
  var node = __lsmFindBySel(sel);
  if (!node) return { handled: false, defaultPrevented: false };
  var chain = __ancestorChain(node.i);
  var dynChain = __dynChainOf(node);
  var handled = false, prevented = false;
  for (var i = 0; i < __lsmListeners.length; i++) {
    var L = __lsmListeners[i];
    if (L.type !== String(type)) continue;
    var targetIdx = -1;
    if (L.node) {
      // 动态节点监听:按对象同一性在动态链上找(0 = 目标本身,-3 仅作哨兵)。
      for (var j = 0; j < dynChain.length; j++) {
        if (dynChain[j] === L.node) { targetIdx = -3; break; }
      }
    } else if (L.sel === "@document" || L.sel === "@window") {
      targetIdx = -2;
    } else {
      for (var j2 = 0; j2 < chain.length; j2++) {
        var n2 = __lsmNodes[chain[j2]];
        if (n2 && __lsmMatchSel(chain[j2], L.sel)) { targetIdx = chain[j2]; break; }
      }
    }
    if (targetIdx === -1) continue;
    var ev = __mkEvent(type);
    ev.target = __mk(node);
    ev.currentTarget = targetIdx === -2 ? document
      : (targetIdx === -3 ? L.node : __mk(__lsmNodes[targetIdx]));
    handled = true;
    try { L.fn(ev); } catch (e) { __cerror("listener " + type + ": " + String((e && e.stack) || e)); }
    if (ev.defaultPrevented) prevented = true;
    if (ev.propagationStopped) break;
  }
  return { handled: handled, defaultPrevented: prevented };
}
// 动态节点自身触发(el.click() 等):沿动态父链冒泡,只跑动态监听。
function __fireDyn(target, type) {
  if (!target) return { handled: false, defaultPrevented: false };
  var chain = [];
  var cur = target, guard = 0;
  while (cur && guard++ < 64) {
    chain.push(cur);
    cur = cur.__parentDyn || null;
  }
  var handled = false, prevented = false;
  for (var i = 0; i < __lsmListeners.length; i++) {
    var L = __lsmListeners[i];
    if (L.type !== String(type) || !L.node) continue;
    var idx = -1;
    for (var j = 0; j < chain.length; j++) {
      if (chain[j] === L.node) { idx = j; break; }
    }
    if (idx === -1) continue;
    var ev = __mkEvent(type);
    ev.target = target;
    ev.currentTarget = L.node;
    handled = true;
    try { L.fn(ev); } catch (e) { __cerror("dyn listener " + type + ": " + String((e && e.stack) || e)); }
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

// ---------- 观察者系列(阶段五第一步) ----------
// 站点前端框架(Vue/React/Ant Design/Element)普遍用 MutationObserver 驱动
// 二次渲染;此前 env_js 里的桩实现回调永不触发,页面会停在「等 DOM 变化」状态。
// 这里按标准语义真实派发:节点稳定键 → 记账 → 选项过滤 → 微任务检查点投递。
// 设计见 docs/自定义浏览器和V8引擎/17-阶段五第一步页面观察与就绪判定设计.md

// 节点稳定键:快照节点按索引(s<i>),动态节点按自增 uid(d<uid>)。
// 快照每轮 refresh 重建包装器,对象同一性跨轮不可用,一律按键比较。
function __lsmKey(node) {
  if (!node) return null;
  if (node.__lsm_node && typeof node.__lsm_node.i === "number") return "s" + node.__lsm_node.i;
  if (typeof node.__lsmUid === "number") return "d" + node.__lsmUid;
  return null;
}
// 属性旧值(不存在时为 null,对齐标准 attributeOldValue 语义)。
function __lsmAttrOld(attrs, key) {
  if (!attrs) return null;
  return Object.prototype.hasOwnProperty.call(attrs, key) ? attrs[key] : null;
}
// 动态节点子树包含(快照树用索引链上溯,动态树用 __kids 递归)。
function __lsmDetachedHas(root, node) {
  var kids = root.__kids || [];
  for (var i = 0; i < kids.length; i++) {
    if (kids[i] === node) return true;
    if (__lsmDetachedHas(kids[i], node)) return true;
  }
  return false;
}
function __lsmIsDescendant(node, root) {
  if (!node || !root) return false;
  var nk = __lsmKey(node), rk = __lsmKey(root);
  if (!nk || !rk) return node === root;
  if (nk === rk) return true;
  if (nk.charAt(0) === "s" && rk.charAt(0) === "s") {
    var idx = Number(nk.slice(1));
    var rid = Number(rk.slice(1));
    var guard = 0;
    while (idx >= 0 && idx < __lsmNodes.length && guard++ < 128) {
      var p = __lsmNodes[idx].parent;
      if (p == null || p < 0 || p >= __lsmNodes.length) break;
      if (p === rid) return true;
      idx = p;
    }
    return false;
  }
  if (nk.charAt(0) === "d" && rk.charAt(0) === "d") return __lsmDetachedHas(root, node);
  return false;
}
// 快照节点的选择器(稳定选择器,如 "#box"):跨 refresh 稳定,索引会漂移。
function __lsmNodeSel(node) {
  if (node && node.__lsm_node && node.__lsm_node.sel) return String(node.__lsm_node.sel);
  return null;
}
// 按选择器在当前快照里找活节点:观察者持有的是旧包装器(矩形已过期),
// 每次判定都要重新解析,否则尺寸/相交永远停在首次观察的值。
function __lsmResolve(node) {
  var sel = __lsmNodeSel(node);
  if (!sel) return node;
  for (var i = 0; i < __lsmNodes.length; i++) {
    if (String(__lsmNodes[i].sel || "") === sel) return __mk(__lsmNodes[i]);
  }
  return node;
}
// 祖先链里是否含指定选择器(子树判定,按选择器而非索引)。
function __lsmHasAncestorSel(node, sel) {
  var idx = node && node.__lsm_node ? node.__lsm_node.i : -1;
  var guard = 0;
  while (idx >= 0 && idx < __lsmNodes.length && guard++ < 128) {
    if (String(__lsmNodes[idx].sel || "") === sel) return true;
    var p = __lsmNodes[idx].parent;
    if (p == null || p < 0 || p >= __lsmNodes.length) break;
    idx = p;
  }
  return false;
}
// 快照节点的 HTML 近似(属性 + 文本):append/prepend 移动快照节点时用。
function __lsmNodeHtml(n) {
  if (!n) return "";
  var tag = String(n.tag || "div").toLowerCase();
  var attrs = n.attrs || {};
  var out = "<" + tag;
  for (var k in attrs) {
    if (!Object.prototype.hasOwnProperty.call(attrs, k)) continue;
    out += " " + k + '="' + String(attrs[k] == null ? "" : attrs[k]).replace(/"/g, "&quot;") + '"';
  }
  out += ">" + String(n.text || "") + "</" + tag + ">";
  return out;
}
// 子节点插入统一记账:动态节点带序列化 HTML;快照节点按「移动」处理
// (先摘除原节点再插副本),否则同一节点会在 DOM 里出现两次。
function __lsmInsertChild(el, targetSel, child, first) {
  if (!child) return;
  try { __lsmRegisterScriptLoad(child); } catch (e) {}
  var html = "";
  if (child.__detached) {
    html = __serEl(child);
  } else if (child.__lsm_node) {
    html = __lsmNodeHtml(child.__lsm_node);
    __push({ type: "remove_node", selector: child.__lsm_node.sel });
  }
  __lsmRec("childList", el, { addedNodes: [child] });
  if (first) __push({ type: "insert_html", selector: targetSel, value: html });
  else __push({ type: "append_html", selector: targetSel, value: html });
}
// 变更记账:统一入口,队列上限 4096(超出丢最旧)。
function __lsmRec(type, target, extra) {
  extra = extra || {};
  var rec = {
    type: String(type),
    target: target || null,
    addedNodes: extra.addedNodes || [],
    removedNodes: extra.removedNodes || [],
    attributeName: extra.attributeName == null ? null : String(extra.attributeName),
    attributeOldValue: extra.attributeOldValue == null ? null : extra.attributeOldValue,
    oldValue: extra.oldValue == null ? null : extra.oldValue,
    timeStamp: __lsmNow,
    seq: ++__lsmMutSeq
  };
  __lsmDomGen++;
  __lsmMutQueue.push(rec);
  if (__lsmMutQueue.length > 4096) __lsmMutQueue.shift();
  return rec;
}

// ---- 宿主侧变更 diff:上一轮快照 ↔ 本轮快照 ----
function __lsmSnapIndex(nodes) {
  var counts = {};
  var map = {};
  for (var i = 0; i < nodes.length; i++) {
    var p = nodes[i].parent;
    if (p != null && p >= 0) counts[p] = (counts[p] || 0) + 1;
  }
  for (var j = 0; j < nodes.length; j++) {
    var n = nodes[j];
    map["s" + n.i] = {
      i: n.i, parent: n.parent,
      attrs: n.attrs || {},
      text: n.text || "",
      value: n.value || "",
      checked: !!n.checked,
      kids: counts[n.i] || 0
    };
  }
  return map;
}
function __lsmDiffSnap(prev, next) {
  var out = [];
  if (!prev) return out;
  for (var key in next) {
    if (!Object.prototype.hasOwnProperty.call(next, key)) continue;
    var cur = next[key], old = prev[key];
    if (!old) {
      // 新节点:挂到父节点上算一次 childList。
      var pid = cur.parent;
      if (pid != null && pid >= 0) out.push({ type: "childList", target: pid });
      continue;
    }
    for (var ak in cur.attrs) {
      if (!Object.prototype.hasOwnProperty.call(cur.attrs, ak)) continue;
      var ov = __lsmAttrOld(old.attrs, ak);
      if (ov !== cur.attrs[ak]) {
        out.push({ type: "attributes", target: cur.i, attributeName: ak, oldValue: ov });
      }
    }
    for (var bk in old.attrs) {
      if (!Object.prototype.hasOwnProperty.call(old.attrs, bk)) continue;
      if (!Object.prototype.hasOwnProperty.call(cur.attrs, bk)) {
        out.push({ type: "attributes", target: cur.i, attributeName: bk, oldValue: old.attrs[bk] });
      }
    }
    if (old.text !== cur.text) {
      out.push({ type: "characterData", target: cur.i, oldValue: old.text });
    }
    if (old.kids !== cur.kids) {
      out.push({ type: "childList", target: cur.i });
    }
  }
  return out;
}
function __lsmApplyDiff(diff) {
  for (var i = 0; i < diff.length; i++) {
    var d = diff[i];
    var node = __lsmNodes[d.target];
    if (!node) continue;
    var el = __mk(node);
    if (d.type === "attributes") {
      __lsmRec("attributes", el, { attributeName: d.attributeName, attributeOldValue: d.oldValue });
    } else if (d.type === "characterData") {
      __lsmRec("characterData", el, { oldValue: d.oldValue });
    } else {
      __lsmRec("childList", el, {});
    }
  }
}

// 活跃观察者计数(登记了目标且未 disconnect):决定是否维护快照 diff 基线。
function __lsmActiveObservers() {
  var n = 0;
  for (var i = 0; i < __lsmMutObservers.length; i++) {
    if (__lsmMutObservers[i]._targets.length > 0) n++;
  }
  return n;
}

// ---- MutationObserver ----
function MutationObserver(cb) {
  if (!(this instanceof MutationObserver)) throw new TypeError("MutationObserver 需要 new 构造");
  this._cb = (typeof cb === "function") ? cb : function () {};
  this._records = [];
  this._targets = [];
  __lsmMutObservers.push(this);
}
MutationObserver.prototype.observe = function (target, options) {
  if (!target) throw new TypeError("observe 需要目标元素");
  options = options || {};
  // 首个观察者登记时建立 diff 基线:从此刻起的宿主侧变化才可见。
  if (!__lsmPrevSnap) __lsmPrevSnap = __lsmSnapIndex(__lsmNodes);
  var filter = options.attributeFilter || null;
  if (filter && !filter.length) filter = null;
  // 标准语义:attributes 仅在显式 true,或给出 attributeFilter/attributeOldValue 时开启。
  var wantAttrs = (options.attributes === true) ||
    (options.attributes !== false && (filter || options.attributeOldValue === true));
  this._targets.push({
    node: target,
    key: __lsmKey(target),
    sel: __lsmNodeSel(target),
    sinceSeq: __lsmMutSeq,
    childList: options.childList !== false,
    attributes: wantAttrs,
    characterData: options.characterData === true || options.characterDataOldValue === true,
    subtree: options.subtree === true,
    filter: filter,
    attributeOldValue: options.attributeOldValue === true,
    characterDataOldValue: options.characterDataOldValue === true
  });
};
MutationObserver.prototype.disconnect = function () {
  this._targets.length = 0;
  this._records.length = 0;
  // 没有观察者时不再维护 diff 基线(大页面每请求全量比对不划算)。
  if (__lsmActiveObservers() === 0) __lsmPrevSnap = null;
};
MutationObserver.prototype.takeRecords = function () {
  var r = this._records;
  this._records = [];
  return r;
};
function __lsmMutMatches(obs, rec) {  for (var t = 0; t < obs._targets.length; t++) {
    var o = obs._targets[t];
    if (!o.key) continue;
    var rk = __lsmKey(rec.target);
    if (!rk) continue;
    // 观察者只应看到登记之后的变更。
    if (rec.seq < o.sinceSeq) continue;
    var hit = (o.key === rk);
    // 索引会随 DOM 变化漂移,选择器才是稳定身份。
    if (!hit && o.sel && o.sel === __lsmNodeSel(rec.target)) hit = true;
    if (!hit && o.subtree) {
      if (o.sel && __lsmHasAncestorSel(rec.target, o.sel)) hit = true;
      else if (!o.sel && __lsmIsDescendant(rec.target, o.node)) hit = true;
    }
    if (!hit) continue;
    if (rec.type === "attributes") {
      if (!o.attributes) continue;
      if (o.filter && o.filter.indexOf(rec.attributeName) < 0) continue;
    } else if (rec.type === "characterData") {
      if (!o.characterData) continue;
    } else if (rec.type === "childList") {
      if (!o.childList) continue;
    } else {
      continue;
    }
    return true;
  }
  return false;
}
// 微任务检查点:把队列里的记录按观察者选项过滤后投递。
// 递归上限 8 轮 / 单轮 32 批:回调再改 DOM 也不许自激死循环。
function __lsmFlushMutations() {
  if (__lsmFlushDepth > 8) { __lsmMutQueue.length = 0; return 0; }
  __lsmFlushDepth++;
  var fired = 0;
  try {
    var guard = 0;
    while (__lsmMutQueue.length > 0 && guard++ < 32) {
      var batch = __lsmMutQueue;
      __lsmMutQueue = [];
      for (var o = 0; o < __lsmMutObservers.length; o++) {
        var obs = __lsmMutObservers[o];
        if (!obs._targets.length) continue;
        var matched = [];
        for (var r = 0; r < batch.length; r++) {
          if (__lsmMutMatches(obs, batch[r])) matched.push(batch[r]);
        }
        if (!matched.length) continue;
        for (var m = 0; m < matched.length; m++) obs._records.push(matched[m]);
        var recs = obs._records;
        obs._records = [];
        fired++;
        try { obs._cb(recs, obs); }
        catch (e) { __cerror("MutationObserver 回调失败: " + String((e && e.stack) || e)); }
      }
    }
  } finally { __lsmFlushDepth--; }
  return fired;
}

// ---- IntersectionObserver / ResizeObserver 公共件 ----
function __lsmThresholds(v) {
  var out = [];
  if (typeof v === "number" && isFinite(v)) out.push(v);
  else if (v && v.length) {
    for (var i = 0; i < v.length; i++) {
      var n = Number(v[i]);
      if (isFinite(n)) out.push(n);
    }
  }
  if (!out.length) out.push(0);
  out.sort(function (a, b) { return a - b; });
  return out;
}
function __lsmCrossed(thresholds, prev, cur) {
  for (var i = 0; i < thresholds.length; i++) {
    var t = thresholds[i];
    if ((prev < t && cur >= t) || (prev >= t && cur < t)) return true;
  }
  return false;
}
function __lsmRectOf(node) {
  try {
    if (node && typeof node.getBoundingClientRect === "function") return node.getBoundingClientRect();
  } catch (e) {}
  return null;
}
function __lsmIntersect(a, b) {
  var l = Math.max(a.left, b.left), t = Math.max(a.top, b.top);
  var r = Math.min(a.right, b.right), bo = Math.min(a.bottom, b.bottom);
  if (r <= l || bo <= t) return null;
  return { left: l, top: t, right: r, bottom: bo, width: r - l, height: bo - t };
}

// ---- IntersectionObserver ----
function IntersectionObserver(cb, options) {
  if (!(this instanceof IntersectionObserver)) throw new TypeError("IntersectionObserver 需要 new 构造");
  this._cb = (typeof cb === "function") ? cb : function () {};
  this._options = options || {};
  this._targets = [];
  this._records = [];
  __lsmIObservers.push(this);
}
IntersectionObserver.prototype.observe = function (target) {
  if (!target) return;
  var key = __lsmKey(target);
  for (var i = 0; i < this._targets.length; i++) {
    if (this._targets[i].key === key) return;
  }
  this._targets.push({
    node: target, key: key, sel: __lsmNodeSel(target),
    thresholds: __lsmThresholds(this._options.threshold),
    prevRatio: 0, prevIntersecting: false, hasPrev: false
  });
};
IntersectionObserver.prototype.unobserve = function (target) {
  var key = __lsmKey(target);
  this._targets = this._targets.filter(function (t) { return t.key !== key; });
};
IntersectionObserver.prototype.disconnect = function () {
  this._targets.length = 0;
  this._records.length = 0;
};
IntersectionObserver.prototype.takeRecords = function () {
  var r = this._records;
  this._records = [];
  return r;
};
function __lsmFlushIntersection() {
  if (!__lsmIObservers.length) return 0;
  var vw = Number(window.innerWidth) || 1280, vh = Number(window.innerHeight) || 720;
  var sx = Number(__lsmMeta.scrollX) || 0, sy = Number(__lsmMeta.scrollY) || 0;
  // 布局矩形是文档坐标,视口矩形按滚动偏移换算到文档坐标后再求交。
  var rootRect = { left: sx, top: sy, right: sx + vw, bottom: sy + vh, width: vw, height: vh };
  var fired = 0;
  for (var o = 0; o < __lsmIObservers.length; o++) {
    var obs = __lsmIObservers[o];
    var entries = [];
    for (var t = 0; t < obs._targets.length; t++) {
      var tg = obs._targets[t];
      // 解析活节点:观察者持有的是旧包装器,矩形会停在首次观察的值。
      var live = __lsmResolve(tg.node);
      var rect = __lsmRectOf(live);
      if (!rect) continue;
      var inter = __lsmIntersect(rect, rootRect);
      var area = (rect.width || 0) * (rect.height || 0);
      var ratio = 0;
      if (inter) ratio = area > 0 ? ((inter.width * inter.height) / area) : 1;
      var intersecting = inter != null;
      var changed = !tg.hasPrev || tg.prevIntersecting !== intersecting ||
        __lsmCrossed(tg.thresholds, tg.prevRatio, ratio);
      if (!changed) continue;
      tg.prevRatio = ratio;
      tg.prevIntersecting = intersecting;
      tg.hasPrev = true;
      entries.push({
        target: live,
        isIntersecting: intersecting,
        intersectionRatio: ratio,
        boundingClientRect: rect,
        intersectionRect: inter || { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 },
        rootBounds: rootRect,
        time: __lsmNow
      });
    }
    if (!entries.length) continue;
    for (var e = 0; e < entries.length; e++) obs._records.push(entries[e]);
    var recs = obs._records;
    obs._records = [];
    fired++;
    try { obs._cb(recs, obs); }
    catch (e2) { __cerror("IntersectionObserver 回调失败: " + String((e2 && e2.stack) || e2)); }
  }
  return fired;
}

// ---- ResizeObserver ----
function ResizeObserver(cb) {
  if (!(this instanceof ResizeObserver)) throw new TypeError("ResizeObserver 需要 new 构造");
  this._cb = (typeof cb === "function") ? cb : function () {};
  this._targets = [];
  this._records = [];
  __lsmRObservers.push(this);
}
ResizeObserver.prototype.observe = function (target) {
  if (!target) return;
  var key = __lsmKey(target);
  for (var i = 0; i < this._targets.length; i++) {
    if (this._targets[i].key === key) return;
  }
  this._targets.push({ node: target, key: key, sel: __lsmNodeSel(target), prevW: null, prevH: null });
};
ResizeObserver.prototype.unobserve = function (target) {
  var key = __lsmKey(target);
  this._targets = this._targets.filter(function (t) { return t.key !== key; });
};
ResizeObserver.prototype.disconnect = function () {
  this._targets.length = 0;
  this._records.length = 0;
};
ResizeObserver.prototype.takeRecords = function () {
  var r = this._records;
  this._records = [];
  return r;
};
function __lsmFlushResize() {
  if (!__lsmRObservers.length) return 0;
  var fired = 0;
  for (var o = 0; o < __lsmRObservers.length; o++) {
    var obs = __lsmRObservers[o];
    var entries = [];
    for (var t = 0; t < obs._targets.length; t++) {
      var tg = obs._targets[t];
      // 解析活节点:尺寸判定必须用当前快照的矩形。
      var live = __lsmResolve(tg.node);
      var rect = __lsmRectOf(live);
      if (!rect) continue;
      var w = rect.width || 0, h = rect.height || 0;
      if (tg.prevW === null) {
        // 首次观察:与标准一致给一条初始尺寸记录。
        tg.prevW = w; tg.prevH = h;
        entries.push(__lsmResizeEntry(live, rect, w, h));
        continue;
      }
      if (tg.prevW === w && tg.prevH === h) continue;
      tg.prevW = w; tg.prevH = h;
      entries.push(__lsmResizeEntry(live, rect, w, h));
    }
    if (!entries.length) continue;
    for (var e = 0; e < entries.length; e++) obs._records.push(entries[e]);
    var recs = obs._records;
    obs._records = [];
    fired++;
    try { obs._cb(recs, obs); }
    catch (e2) { __cerror("ResizeObserver 回调失败: " + String((e2 && e2.stack) || e2)); }
  }
  return fired;
}
function __lsmResizeEntry(target, rect, w, h) {
  var contentRect = { x: rect.left || 0, y: rect.top || 0, width: w, height: h };
  var box = [{ inlineSize: w, blockSize: h }];
  return {
    target: target,
    contentRect: contentRect,
    borderBoxSize: box,
    contentBoxSize: box,
    devicePixelContentBoxSize: [{ inlineSize: w, blockSize: h }]
  };
}

// 组合入口:三类观察者一次派发(宿主请求收口与虚拟时钟批次共用)。
function __lsmFlushObservers() {
  try { __lsmFlushMutations(); } catch (e) { __cerror("观察者派发失败: " + String(e)); }
  try { __lsmFlushIntersection(); } catch (e2) { __cerror("IntersectionObserver 派发失败: " + String(e2)); }
  try { __lsmFlushResize(); } catch (e3) { __cerror("ResizeObserver 派发失败: " + String(e3)); }
}

// window 暴露:站点普遍写 `new window.MutationObserver(...)`。
try {
  __lsmWin.MutationObserver = MutationObserver;
  __lsmWin.IntersectionObserver = IntersectionObserver;
  __lsmWin.ResizeObserver = ResizeObserver;
} catch (e) {}

// ---------- 宿主驱动入口 ----------
function __lsmErrText(e) {
  if (e === null || e === undefined) return "unknown error";
  var parts = [];
  try { if (e.name) parts.push(String(e.name)); } catch (x) {}
  try { if (e.message) parts.push(String(e.message)); } catch (x) {}
  try { if (e.stack) parts.push(String(e.stack)); } catch (x) {}
  try { if (e.lineNumber !== undefined) parts.push("line=" + e.lineNumber); } catch (x) {}
  if (!parts.length) { try { parts.push(String(e)); } catch (x) { parts.push("unprintable error"); } }
  return parts.join(" ");
}
// ---------- ToObject 调试探针(LAEW_JS_TRACE_TOOBJ=1,默认零输出) ----------
// Boa 对 `Object.keys(null)` / `Object.defineProperty(undefined,...)` / `for-in null`
// 抛 "cannot convert 'null' or 'undefined' to object",但运行时不带源码位置。
// 开关打开时在这些内建入口记录调用,用于定位压缩脚本里的具体调用点。
(function () {
  if (!globalThis.__lsmTraceToObjInstall) return;
  // where: "arg0" = 第一个参数做 ToObject, "this" = this 值做 ToObject。
  function wrap(name, owner, label, where) {
    try {
      var orig = owner[name];
      if (typeof orig !== "function") return;
      var w = function (v) {
        "use strict";
        var bad = false;
        try {
          bad = where === "this"
            ? (this === null || this === undefined)
            : (v === null || v === undefined);
        } catch (e) {}
        try {
          return orig.apply(this, arguments);
        } catch (e) {
          // 只在**真的抛错**时记录。早前是「调用即记」,压缩脚本里 `Object.create(null)`
          // 这类完全正常的调用会把控制台刷爆(实测百度地图一轮 300+ 条),
          // 真正的抛错点反被挤到采集上限之外,开关等于白开。
          if (globalThis.__lsmTraceToObj) {
            var st = "";
            try { st = String((new Error()).stack || e); } catch (x) { st = String(e); }
            __cerror("[toobj] " + label + " where=" + where + " bad=" + bad +
                     " arg0=" + String(v) + " | " + st);
          }
          throw e;
        }
      };
      w.__lsmOrig = orig;
      owner[name] = w;
    } catch (e) {}
  }
  var O = Object;
  wrap("keys", O, "Object.keys", "arg0");
  wrap("getOwnPropertyNames", O, "Object.getOwnPropertyNames", "arg0");
  wrap("getOwnPropertySymbols", O, "Object.getOwnPropertySymbols", "arg0");
  wrap("assign", O, "Object.assign", "arg0");
  wrap("defineProperty", O, "Object.defineProperty", "arg0");
  wrap("defineProperties", O, "Object.defineProperties", "arg0");
  wrap("getOwnPropertyDescriptor", O, "Object.getOwnPropertyDescriptor", "arg0");
  wrap("getOwnPropertyDescriptors", O, "Object.getOwnPropertyDescriptors", "arg0");
  wrap("setPrototypeOf", O, "Object.setPrototypeOf", "arg0");
  wrap("freeze", O, "Object.freeze", "arg0");
  wrap("seal", O, "Object.seal", "arg0");
  wrap("preventExtensions", O, "Object.preventExtensions", "arg0");
  wrap("getPrototypeOf", O, "Object.getPrototypeOf", "arg0");
  wrap("create", O, "Object.create", "arg0");
  wrap("entries", O, "Object.entries", "arg0");
  wrap("values", O, "Object.values", "arg0");
  wrap("fromEntries", O, "Object.fromEntries", "arg0");
  wrap("hasOwnProperty", O.prototype, "hasOwnProperty", "this");
  wrap("propertyIsEnumerable", O.prototype, "propertyIsEnumerable", "this");
  wrap("isPrototypeOf", O.prototype, "isPrototypeOf", "this");
  wrap("valueOf", O.prototype, "Object.valueOf", "this");
  var A = Array.prototype;
  ["forEach", "map", "filter", "some", "every", "reduce", "reduceRight", "indexOf",
   "lastIndexOf", "join", "slice", "splice", "concat", "reverse", "sort", "pop", "shift",
   "push", "unshift", "find", "findIndex", "includes"].forEach(function (m) {
    wrap(m, A, "Array." + m, "this");
  });
  var S = String.prototype;
  ["split", "slice", "substring", "substr", "charAt", "charCodeAt", "indexOf", "lastIndexOf",
   "replace", "match", "search", "toLowerCase", "toUpperCase", "trim", "concat"].forEach(function (m) {
    wrap(m, S, "String." + m, "this");
  });
  var R = RegExp.prototype;
  ["exec", "test"].forEach(function (m) {
    wrap(m, R, "RegExp." + m, "this");
  });
  var D = Date.prototype;
  ["getTime", "getFullYear", "getMonth", "getDate", "getDay"].forEach(function (m) {
    wrap(m, D, "Date." + m, "this");
  });
})();

// 当前 AMD 装载器形态。宿主据此决定是否对脚本做 define 依赖数组补齐:
// FIS 系装载器只认 `define(id, factory)` 两参同步注册,补齐后的三参形态会被
// 它转交给 RequireJS 分支,模块进 globalDefQueue 而永不进同步注册表
// (百度地图 pano 模块 "has not been loaded yet")。
// 调试:define 工厂体语句标记(LAEW_JS_TRACE_STMT=1 时由宿主注入)。
var __lsmLastStmt = "";
function __lsmStmtMark(head) {
  __lsmLastStmt = String(head);
  if (globalThis.__lsmTraceStmt) __cerror("[stmt] " + __lsmLastStmt);
}
function __lsmAmdKind() {
  try {
    __lsmEnsureSubstr();
    if (typeof require === "undefined" || !require) return "none";
    var r = require;
    if (r.s && r.s.contexts) return "requirejs";
    if (typeof r.loadJs === "function" || typeof r.loadCss === "function" || typeof r.alias === "function") {
      __lsmWrapAmdDefine();
      return "fis";
    }
    if (typeof r.defined === "function") return "minimal";
    return "unknown";
  } catch (e) { return "none"; }
}
// 给同步 AMD 装载器(FIS)的 define 套一层归属包装:工厂抛错时记录模块 id。
// 不包装时异常只会记到「调用 require 的那段脚本」名下,且 FIS 会把失败模块
// 缓存成空 exports,后续 require 静默返回半成品(实测百度地图 tangram 工厂
// 抛错 → window.T 永不赋值 → messageCenter 等整层 ReferenceError)。
var __lsmAmdWrapped = false;
function __lsmWrapAmdDefine() {
  if (__lsmAmdWrapped) return;
  if (typeof define !== "function") return;
  var orig = define;
  var wrapped = function (id, deps, factory) {
    if (typeof id === "string" && typeof deps === "function") {
      var mid = id;
      var f = deps;
      return orig(mid, function () {
        try { return f.apply(this, arguments); }
        catch (e) { __cerror("AMD 模块执行失败 " + mid + ": " + __lsmErrText(e) + " | last=" + __lsmLastStmt); throw e; }
      });
    }
    return orig.apply(this, arguments);
  };
  // UMD 用 `"function" == typeof define && define.amd` 选择导出通道。
  // 原加载器没有 amd 时不能凭空补 {}，否则脚本不再注册浏览器全局。
  if (orig.amd !== undefined) wrapped.amd = orig.amd;
  wrapped.__lsmWrapped = true;
  try { window.define = wrapped; } catch (e) {}
  try { define = wrapped; } catch (e) {}
  __lsmAmdWrapped = true;
}
function __lsmRunScript(src) {
  try { (0, eval)(String(src)); }
  catch (e) {
    // 带上出错脚本片段:只报 "TypeError: ..." 无法定位到具体哪段脚本炸了。
    var head = String(src).replace(/\s+/g, " ").slice(0, 700);
    __cerror(__lsmErrText(e) + " @ script: " + head);
  }
  return true;
}
function __lsmEvalUser(src) {
  var __err = null, __r;
  try {
    try { __r = (0, eval)("(" + src + "\n)"); }
    catch (e1) {
      if (e1 && e1.name === "SyntaxError") {
        // 含 `return` 的语句体:用**间接 eval 包 IIFE**,不要走 `new Function`。
        // Boa 0.20 的 `new Function` 编译出的函数体里只要有 `class` 声明,执行时
        // 必 panic(`must be declarative environment`),整条页面 JS 线程被打死、
        // 页面全局状态全丢(实测百度地图任务中途崩)。间接 eval 路径对 class 正常。
        if (/\breturn\b/.test(src)) { __r = (0, eval)("(function(){\n" + src + "\n})()"); }
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
// 宿主侧脚本执行包装:间接 eval(全局作用域)失败时把 e.stack(含行列)回传,
// 否则只拿到 "TypeError: xxx" 一句,几百 KB 的压缩脚本无从定位。
function __lsmTraceAlogException() {
  if (!globalThis.__lsmTraceToObj || typeof Proxy !== "function") return;
  try {
    if (typeof alog !== "function" || alog.__lsmTraceWrapped) return;
    var wrapped = new Proxy(alog, {
      apply: function (target, thisArg, args) {
        if (args[0] === "exception.fire") {
          try { __cerror("[alog exception] " + __str(args)); } catch (e) {}
        }
        return target.apply(thisArg, args);
      }
    });
    wrapped.__lsmTraceWrapped = true;
    alog = wrapped;
    window.alog = wrapped;
  } catch (e) {}
}
function __lsmRunScript(code) {
  __lsmTraceAlogException();
  try { (0, eval)(String(code)); return __lsmCollect({ ok: true, value: null }); }
  catch (e) {
    // Boa 的 e.stack 只有调用帧、不含 "TypeError: xxx" 消息行;
    // 只回传 stack 会丢失错误本体(zhipin jQuery/Vue 失败只有帧无从排查)。
    // 消息与栈都保留:消息行在前,与浏览器 DevTools 习惯一致。
    var head = "";
    try { head = String(e); } catch (se) { head = "(String(e) 抛错)"; }
    var stack = "";
    try { stack = (e && e.stack) ? String(e.stack) : ""; } catch (se2) { stack = ""; }
    var msg = stack ? (head + "\n" + stack) : head;
    return __lsmCollect({ ok: false, value: null, error: msg });
  }
}
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
  // 请求收口派发观察者:回调产生的 DOM 动作在本轮动作表里,宿主一次回写。
  __lsmFlushObservers();
  // 动态节点挂载后的属性变更合并下发(每元素每属性每轮一条)。
  __lsmFlushDynAttrs();
  return JSON.stringify({
    ok: ("ok" in ov) ? !!ov.ok : __lsmFatal === null,
    value: ("value" in ov) ? ov.value : null,
    type: ("type" in ov) ? ov.type : "undefined",
    error: ("error" in ov) ? ov.error : __lsmFatal,
    actions: __lsmActions,
    console: __lsmConsole,
    handled: __lsmDispatchResult.handled,
    defaultPrevented: __lsmDispatchResult.defaultPrevented,
    webglAsked: Number(window.__lsmWebGLAsked) || 0
  });
}
__lsmFatal = null;
"##;

/// 完整 PRELUDE 源码:主体 + 共享选择器引擎 + 共享标准全局。
///
/// 选择器引擎与标准全局抽成独立常量,阶段一(`js.rs`)与阶段二共用,
/// 避免两套 PRELUDE 各改各的、行为悄悄漂移(历史上阶段二丢了 `el.title`)。
pub static PRELUDE: once_cell::sync::Lazy<String> = once_cell::sync::Lazy::new(|| {
    format!(
        "{}\n{}\n{}",
        PRELUDE_BODY,
        super::env_js::SELECTOR_JS,
        super::env_js::GLOBALS_JS
    )
});
