//! IndexedDB 最小内存实现(PRELUDE 注入件)。
//!
//! 2026-11-20 闲鱼真机根因:内核缺 `window.indexedDB`,站点本地缓存 SDK
//! 探测 `if (!window.indexedDB) throw new Error('该浏览器不支持indexedDB')`
//! 且异常未被 catch,连锁 `TypeError: cannot convert ...` 与
//! `MessagePort onmessage 失败`,React 动态子树(首页信息流/搜索商品卡)
//! 渲染中断 —— mtop feed 接口 200+206KB 数据在手也画不出来。
//!
//! 实现形态:纯 JS(ES5 风格,与 PRELUDE 主体一致)、全内存
//! (`__lsmIDBDbs` 随 JS 线程存活,跨导航持久、进程退出即失)、
//! 异步回调经 `setTimeout(fn, 0)` 排队(真实浏览器异步事件语义)。
//! fail-soft:内部异常一律走 request.onerror + `__cerror` 记账,
//! 永不把异常 throw 回站点初始化链(补标准 API 的既有铁律:
//! 站点 polyfill/SDK 只做存在性探测就直接用,语义够用即可)。
//!
//! API 面:IDBFactory(open/deleteDatabase/databases/cmp)、IDBDatabase、
//! IDBTransaction、IDBObjectStore(get/getAll/getAllKeys/put/add/delete/
//! clear/count/index/openCursor)、IDBIndex、IDBCursor、IDBKeyRange、
//! 轻量 DOMException。单连接单线程场景,无 onblocked/多连接仲裁。

pub const IDB_JS: &str = r##"
// ---------- IndexedDB 最小内存实现 ----------
// 数据唯一真源:__lsmIDBDbs = { 库名: {version, stores: {店名: {keyPath,
// autoIncrement, nextAutoId, data: {归一键: {key, val}}, indexes: {名: {keyPath, unique}}}}, closed} }
var __lsmIDBDbs = {};
var __lsmIDBSeq = 0;

function __lsmIDBErr(name, msg) {
  var e = new Error(msg || name);
  e.name = name;
  return e;
}

// 键归一化:数字/字符串/Date 统一进同一可排序字符串空间(前缀分域,
// 同域内数字按数值、字符串按字典序 —— 与 IndexedDB 键序近似)。
function __lsmIDBNormKey(k) {
  if (k instanceof Date) return "D" + __lsmIDBPadNum(k.getTime());
  if (typeof k === "number" && isFinite(k)) return "N" + __lsmIDBPadNum(k);
  if (typeof k === "string") return "S" + k;
  // 数组键(复合主键)逐项展开
  if (k && typeof k === "object" && typeof k.length === "number") {
    var parts = [];
    for (var i = 0; i < k.length; i++) parts.push(__lsmIDBNormKey(k[i]));
    return "A" + String.fromCharCode(1) + parts.join(String.fromCharCode(1));
  }
  return "S" + String(k);
}
function __lsmIDBPadNum(n) {
  // 数字键排序近似:定宽十六进制,负数取反保证单调
  var neg = n < 0;
  var v = neg ? -n : n;
  var hex = "0000000000000000" + v.toString(16);
  hex = hex.slice(hex.length - 16);
  return (neg ? "0" : "1") + hex;
}

// keyPath 提取:支持 "a.b.c" 嵌套与 ["a","b"] 复合;取不到返回 undefined
function __lsmIDBExtractKey(val, keyPath) {
  if (keyPath == null) return undefined;
  var paths = Object.prototype.toString.call(keyPath) === "[object Array]" ? keyPath : [keyPath];
  var out = [];
  for (var i = 0; i < paths.length; i++) {
    var segs = String(paths[i]).split(".");
    var cur = val;
    for (var j = 0; j < segs.length; j++) {
      if (cur == null) { cur = undefined; break; }
      cur = cur[segs[j]];
    }
    out.push(cur);
  }
  return out.length === 1 ? out[0] : out;
}

// 轻量 DOMStringList(objectStoreNames / indexNames 用)
function __lsmIDBNameList(names) {
  var list = { length: names.length, item: function (i) { return names[i] || null; },
               contains: function (n) { return names.indexOf(n) >= 0; } };
  for (var i = 0; i < names.length; i++) list[i] = names[i];
  return list;
}

function __lsmIDBFire(handlers, ev) {
  // 同步回调异常不外抛(fail-soft),记 console 台账
  for (var i = 0; i < handlers.length; i++) {
    try { handlers[i](ev); } catch (e) { __cerror("IndexedDB 回调异常: " + String((e && e.stack) || e)); }
  }
}

function IDBRequest() {
  this.__handlers = { success: [], error: [] };
  this.result = undefined;
  this.error = null;
  this.readyState = "pending";
  this.source = null;
  this.transaction = null;
  this.onsuccess = null;
  this.onerror = null;
}
IDBRequest.prototype.addEventListener = function (type, fn) {
  if (fn && this.__handlers[type]) this.__handlers[type].push(fn);
};
IDBRequest.prototype.removeEventListener = function (type, fn) {
  var arr = this.__handlers[type] || [];
  var i = arr.indexOf(fn);
  if (i >= 0) arr.splice(i, 1);
};
IDBRequest.prototype.__succeed = function (result) {
  var self = this;
  this.result = result;
  this.readyState = "done";
  setTimeout(function () { self.__dispatch("success"); }, 0);
};
IDBRequest.prototype.__fail = function (err) {
  var self = this;
  this.error = err;
  this.readyState = "done";
  setTimeout(function () { self.__dispatch("error"); }, 0);
};
IDBRequest.prototype.__dispatch = function (type) {
  var ev = { type: type, target: this, currentTarget: this };
  if (this["on" + type]) __lsmIDBFire([this["on" + type]], ev);
  __lsmIDBFire(this.__handlers[type], ev);
  // 请求收尾驱动事务 oncomplete(计数归零后异步补一拍)
  var tx = this.transaction;
  if (tx && tx.__pending > 0) {
    tx.__pending--;
    if (tx.__pending === 0 && !tx.__done) {
      tx.__done = true;
      setTimeout(function () { tx.__complete(); }, 0);
    }
  }
};

function IDBTransaction(db, storeNames, mode) {
  this.__db = db;
  this.__stores = storeNames.slice();
  this.__pending = 0;
  this.__done = false;
  this.__aborted = false;
  this.mode = mode || "readonly";
  this.oncomplete = null;
  this.onerror = null;
  this.onabort = null;
  this.db = db;
}
IDBTransaction.prototype.objectStore = function (name) {
  var meta = this.__db.__meta.stores[name];
  if (!meta) throw __lsmIDBErr("NotFoundError", "objectStore 不存在: " + name);
  var self = this;
  var store = new IDBObjectStore(meta, this);
  return store;
};
IDBTransaction.prototype.abort = function () {
  if (this.__done) return;
  this.__aborted = true;
  this.__done = true;
  var self = this;
  setTimeout(function () {
    var ev = { type: "abort", target: self };
    if (self.onabort) __lsmIDBFire([self.onabort], ev);
  }, 0);
};
IDBTransaction.prototype.__complete = function () {
  if (this.__aborted) return;
  var ev = { type: "complete", target: this };
  if (this.oncomplete) __lsmIDBFire([this.oncomplete], ev);
};
// 空事务也要异步 complete:创建时排一拍检查,若期间同步发了请求则由请求收尾接管
IDBTransaction.prototype.__armEmpty = function () {
  var self = this;
  setTimeout(function () {
    if (self.__pending === 0 && !self.__done) {
      self.__done = true;
      self.__complete();
    }
  }, 0);
};

function IDBObjectStore(meta, tx) {
  this.__meta = meta;
  this.__tx = tx;
  this.name = meta.name;
  this.keyPath = meta.keyPath || null;
  this.autoIncrement = !!meta.autoIncrement;
  this.indexNames = __lsmIDBNameList(Object.keys(meta.indexes));
}
IDBObjectStore.prototype.__req = function (source) {
  var req = new IDBRequest();
  req.source = source || this;
  req.transaction = this.__tx;
  this.__tx.__pending++;
  return req;
};
IDBObjectStore.prototype.__checkActive = function () {
  if (this.__tx.__done || this.__tx.__aborted) throw __lsmIDBErr("TransactionInactiveError", "事务已结束");
};
IDBObjectStore.prototype.__storeValueKey = function (value, key) {
  var k = (key !== undefined) ? key : __lsmIDBExtractKey(value, this.__meta.keyPath);
  if (k === undefined && this.__meta.autoIncrement) {
    k = ++this.__meta.nextAutoId;
  }
  if (k === undefined) throw __lsmIDBErr("DataError", "无法从 value 提取键(keyPath 缺失且非自增)");
  return k;
};
IDBObjectStore.prototype.put = function (value, key) {
  this.__checkActive();
  var req = this.__req();
  var meta = this.__meta;
  try {
    var k = this.__storeValueKey(value, key);
    if (this.keyPath && key === undefined) {
      // inline key 回写(value 上带键,真实浏览器同语义)
      var paths = Object.prototype.toString.call(this.keyPath) === "[object Array]" ? this.keyPath : [this.keyPath];
      if (paths.length === 1) {
        var segs = String(paths[0]).split(".");
        var cur = value;
        for (var j = 0; j < segs.length - 1; j++) {
          if (cur[segs[j]] == null) cur[segs[j]] = {};
          cur = cur[segs[j]];
        }
        cur[segs[segs.length - 1]] = k;
      }
    }
    meta.data[__lsmIDBNormKey(k)] = { key: k, val: value };
    req.__succeed(k);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBObjectStore.prototype.add = function (value, key) {
  this.__checkActive();
  var req = this.__req();
  var meta = this.__meta;
  try {
    var k = this.__storeValueKey(value, key);
    var nk = __lsmIDBNormKey(k);
    if (metaHas(meta, nk)) { req.__fail(__lsmIDBErr("ConstraintError", "键已存在")); return req; }
    meta.data[nk] = { key: k, val: value };
    req.__succeed(k);
  } catch (e) { req.__fail(e); }
  return req;
};
function metaHas(meta, nk) { return Object.prototype.hasOwnProperty.call(meta.data, nk); }
IDBObjectStore.prototype.get = function (key) {
  this.__checkActive();
  var req = this.__req();
  try {
    var rec = this.__meta.data[__lsmIDBNormKey(key)];
    req.__succeed(rec ? rec.val : undefined);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBObjectStore.prototype.getKey = function (key) {
  this.__checkActive();
  var req = this.__req();
  try {
    var rec = this.__meta.data[__lsmIDBNormKey(key)];
    req.__succeed(rec ? rec.key : undefined);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBObjectStore.prototype.delete = function (key) {
  this.__checkActive();
  var req = this.__req();
  try { delete this.__meta.data[__lsmIDBNormKey(key)]; req.__succeed(undefined); }
  catch (e) { req.__fail(e); }
  return req;
};
IDBObjectStore.prototype.clear = function () {
  this.__checkActive();
  var req = this.__req();
  try { this.__meta.data = {}; req.__succeed(undefined); }
  catch (e) { req.__fail(e); }
  return req;
};
IDBObjectStore.prototype.count = function (key) {
  this.__checkActive();
  var req = this.__req();
  try {
    if (key === undefined) { req.__succeed(this.__sortedRecords().length); return req; }
    var range = __lsmIDBKeyRangeOf(key);
    req.__succeed(this.__sortedRecords().filter(function (r) { return range ? range.__containsNorm(r.norm) : true; }).length);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBObjectStore.prototype.getAll = function (key, count) {
  this.__checkActive();
  var req = this.__req();
  try {
    var range = key === undefined ? null : __lsmIDBKeyRangeOf(key);
    var recs = this.__sortedRecords().filter(function (r) { return range ? range.__containsNorm(r.norm) : true; });
    var vals = recs.map(function (r) { return r.rec.val; });
    if (typeof count === "number" && count >= 0) vals = vals.slice(0, count);
    req.__succeed(vals);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBObjectStore.prototype.getAllKeys = function (key, count) {
  this.__checkActive();
  var req = this.__req();
  try {
    var range = key === undefined ? null : __lsmIDBKeyRangeOf(key);
    var recs = this.__sortedRecords().filter(function (r) { return range ? range.__containsNorm(r.norm) : true; });
    var keys = recs.map(function (r) { return r.rec.key; });
    if (typeof count === "number" && count >= 0) keys = keys.slice(0, count);
    req.__succeed(keys);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBObjectStore.prototype.__sortedRecords = function () {
  var data = this.__meta.data;
  var norms = Object.keys(data).sort();
  var out = [];
  for (var i = 0; i < norms.length; i++) out.push({ norm: norms[i], rec: data[norms[i]] });
  return out;
};
IDBObjectStore.prototype.createIndex = function (name, keyPath, opts) {
  if (this.__tx.mode !== "versionchange") throw __lsmIDBErr("InvalidStateError", "createIndex 仅版本变更事务内可用");
  var o = opts || {};
  this.__meta.indexes[name] = { keyPath: keyPath, unique: !!o.unique };
  this.indexNames = __lsmIDBNameList(Object.keys(this.__meta.indexes));
  return new IDBIndex(this.__meta, name, this.__tx, this);
};
IDBObjectStore.prototype.index = function (name) {
  var im = this.__meta.indexes[name];
  if (!im) throw __lsmIDBErr("NotFoundError", "index 不存在: " + name);
  return new IDBIndex(this.__meta, name, this.__tx, this);
};
IDBObjectStore.prototype.deleteIndex = function (name) {
  if (this.__tx.mode !== "versionchange") throw __lsmIDBErr("InvalidStateError", "deleteIndex 仅版本变更事务内可用");
  delete this.__meta.indexes[name];
  this.indexNames = __lsmIDBNameList(Object.keys(this.__meta.indexes));
};
IDBObjectStore.prototype.openCursor = function (range, direction) {
  this.__checkActive();
  var req = this.__req();
  try {
    var r = range ? __lsmIDBKeyRangeOf(range) : null;
    var recs = this.__sortedRecords().filter(function (x) { return r ? r.__containsNorm(x.norm) : true; });
    if (direction === "prev" || direction === "prevunique") recs = recs.slice().reverse();
    req.__cursorRecords = recs;
    req.__cursorPos = 0;
    req.__cursorStore = this;
    __lsmIDBCursorStep(req);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBObjectStore.prototype.openKeyCursor = function (range, direction) {
  return this.openCursor(range, direction);
};

function __lsmIDBCursorStep(req) {
  var recs = req.__cursorRecords;
  var pos = req.__cursorPos;
  var result = null;
  if (pos < recs.length) {
    result = new IDBCursor(req, recs, pos);
  }
  req.result = result;
  req.readyState = "done";
  setTimeout(function () { req.__dispatch("success"); }, 0);
}

function IDBCursor(req, recs, pos) {
  this.__req = req;
  this.__recs = recs;
  this.__pos = pos;
  var rec = recs[pos].rec;
  this.key = rec.key;
  this.value = rec.val;
  this.primaryKey = rec.key;
  this.source = req.source;
  this.direction = "next";
  this.request = req;
}
IDBCursor.prototype.continue = function (key) {
  if (this.__req.__cursorPos === null) throw __lsmIDBErr("InvalidStateError", "cursor 已到尾");
  this.__req.__cursorPos++;
  if (key !== undefined) {
    // 跳到 >= key 的位置
    var target = __lsmIDBNormKey(key);
    var recs = this.__req.__cursorRecords;
    while (this.__req.__cursorPos < recs.length && recs[this.__req.__cursorPos].norm < target) this.__req.__cursorPos++;
  }
  __lsmIDBCursorStep(this.__req);
};
IDBCursor.prototype.advance = function (count) {
  if (!(count > 0)) throw __lsmIDBErr("TypeError", "advance 次数须为正");
  this.__req.__cursorPos += count;
  __lsmIDBCursorStep(this.__req);
};
IDBCursor.prototype.update = function (value) {
  var req = new IDBRequest();
  req.source = this.source;
  req.transaction = this.__req.transaction;
  try {
    this.__recs[this.__pos].rec.val = value;
    req.__succeed(this.key);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBCursor.prototype["delete"] = function () {
  var req = new IDBRequest();
  req.source = this.source;
  req.transaction = this.__req.transaction;
  try {
    delete this.__req.__cursorStore.__meta.data[this.__recs[this.__pos].norm];
    req.__succeed(undefined);
  } catch (e) { req.__fail(e); }
  return req;
};

function IDBIndex(meta, name, tx, store) {
  this.__meta = meta;
  this.__im = meta.indexes[name];
  this.__tx = tx;
  this.__store = store;
  this.name = name;
  this.keyPath = this.__im.keyPath;
  this.unique = !!this.__im.unique;
  this.objectStore = store;
}
IDBIndex.prototype.__matched = function () {
  var im = this.__im;
  var out = [];
  var norms = Object.keys(this.__meta.data).sort();
  for (var i = 0; i < norms.length; i++) {
    var rec = this.__meta.data[norms[i]];
    var ik = __lsmIDBExtractKey(rec.val, im.keyPath);
    if (ik !== undefined) out.push({ norm: __lsmIDBNormKey(ik), rec: rec });
  }
  out.sort(function (a, b) { return a.norm < b.norm ? -1 : (a.norm > b.norm ? 1 : 0); });
  return out;
};
IDBIndex.prototype.__req = function () {
  var req = new IDBRequest();
  req.source = this;
  req.transaction = this.__tx;
  this.__tx.__pending++;
  return req;
};
IDBIndex.prototype.get = function (key) {
  var req = this.__req();
  try {
    var target = __lsmIDBNormKey(key);
    var hits = this.__matched().filter(function (m) { return m.norm === target; });
    req.__succeed(hits.length ? hits[0].rec.val : undefined);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBIndex.prototype.getKey = function (key) {
  var req = this.__req();
  try {
    var target = __lsmIDBNormKey(key);
    var hits = this.__matched().filter(function (m) { return m.norm === target; });
    req.__succeed(hits.length ? hits[0].rec.key : undefined);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBIndex.prototype.getAll = function (key, count) {
  var req = this.__req();
  try {
    var matched = this.__matched();
    if (key !== undefined) {
      var range = __lsmIDBKeyRangeOf(key);
      matched = matched.filter(function (m) { return range ? range.__containsNorm(m.norm) : true; });
    }
    var vals = matched.map(function (m) { return m.rec.val; });
    if (typeof count === "number" && count >= 0) vals = vals.slice(0, count);
    req.__succeed(vals);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBIndex.prototype.getAllKeys = function (key, count) {
  var req = this.__req();
  try {
    var matched = this.__matched();
    if (key !== undefined) {
      var range = __lsmIDBKeyRangeOf(key);
      matched = matched.filter(function (m) { return range ? range.__containsNorm(m.norm) : true; });
    }
    var keys = matched.map(function (m) { return m.rec.key; });
    if (typeof count === "number" && count >= 0) keys = keys.slice(0, count);
    req.__succeed(keys);
  } catch (e) { req.__fail(e); }
  return req;
};
IDBIndex.prototype.count = function (key) {
  var req = this.__req();
  try {
    var matched = this.__matched();
    if (key !== undefined) {
      var range = __lsmIDBKeyRangeOf(key);
      matched = matched.filter(function (m) { return range ? range.__containsNorm(m.norm) : true; });
    }
    req.__succeed(matched.length);
  } catch (e) { req.__fail(e); }
  return req;
};

// IDBKeyRange:统一带 __containsNorm(归一化键空间)供 store/index 过滤
function IDBKeyRange(lower, upper, lowerOpen, upperOpen) {
  this.lower = lower;
  this.upper = upper;
  this.lowerOpen = !!lowerOpen;
  this.upperOpen = !!upperOpen;
  var ln = lower === undefined ? null : __lsmIDBNormKey(lower);
  var un = upper === undefined ? null : __lsmIDBNormKey(upper);
  this.__ln = ln;
  this.__un = un;
  this.__containsNorm = function (n) {
    if (ln !== null) { if (this.lowerOpen ? n <= ln : n < ln) return false; }
    if (un !== null) { if (this.upperOpen ? n >= un : n > un) return false; }
    return true;
  };
}
IDBKeyRange.prototype.includes = function (key) { return this.__containsNorm(__lsmIDBNormKey(key)); };
IDBKeyRange.only = function (v) { return new IDBKeyRange(v, v, false, false); };
IDBKeyRange.lowerBound = function (v, open) { return new IDBKeyRange(v, undefined, !!open, false); };
IDBKeyRange.upperBound = function (v, open) { return new IDBKeyRange(undefined, v, false, !!open); };
IDBKeyRange.bound = function (l, u, lo, uo) { return new IDBKeyRange(l, u, !!lo, !!uo); };

// 参数容错:openCursor/getAll 传进来的可能是 IDBKeyRange 也可能是裸键
function __lsmIDBKeyRangeOf(key) {
  if (key && key.__containsNorm) return key;
  return IDBKeyRange.only(key);
}

function IDBDatabase(meta) {
  this.__meta = meta;
  this.name = meta.name;
  this.version = meta.version;
  this.onversionchange = null;
  this.onclose = null;
  this.__closed = false;
}
Object.defineProperty(IDBDatabase.prototype, "objectStoreNames", {
  get: function () { return __lsmIDBNameList(Object.keys(this.__meta.stores)); }
});
IDBDatabase.prototype.createObjectStore = function (name, opts) {
  if (this.__meta.stores[name]) throw __lsmIDBErr("ConstraintError", "objectStore 已存在: " + name);
  var o = opts || {};
  this.__meta.stores[name] = { name: name, keyPath: o.keyPath, autoIncrement: !!o.autoIncrement,
                               nextAutoId: 0, data: {}, indexes: {} };
  return new IDBObjectStore(this.__meta.stores[name], this.__vcTx);
};
IDBDatabase.prototype.deleteObjectStore = function (name) {
  if (!this.__meta.stores[name]) throw __lsmIDBErr("NotFoundError", "objectStore 不存在: " + name);
  delete this.__meta.stores[name];
};
IDBDatabase.prototype.transaction = function (storeNames, mode) {
  if (this.__closed) throw __lsmIDBErr("InvalidStateError", "数据库已关闭");
  var names = Object.prototype.toString.call(storeNames) === "[object Array]" ? storeNames.slice() : [String(storeNames)];
  for (var i = 0; i < names.length; i++) {
    if (!this.__meta.stores[names[i]]) throw __lsmIDBErr("NotFoundError", "objectStore 不存在: " + names[i]);
  }
  var m = (mode === "readwrite" || mode === "versionchange") ? mode : "readonly";
  var tx = new IDBTransaction(this, names, m);
  tx.__armEmpty();
  return tx;
};
IDBDatabase.prototype.close = function () { this.__closed = true; };

// 工厂对象:站点探测的入口
var __lsmIndexedDB = {
  cmp: function (a, b) {
    var na = __lsmIDBNormKey(a), nb = __lsmIDBNormKey(b);
    return na < nb ? -1 : (na > nb ? 1 : 0);
  },
  open: function (name, version) {
    var req = new IDBRequest();
    req.source = this;
    setTimeout(function () {
      try {
        var meta = __lsmIDBDbs[name];
        if (!meta) {
          meta = { name: name, version: version || 1, stores: {} };
          __lsmIDBDbs[name] = meta;
          var db0 = new IDBDatabase(meta);
          // 版本变更事务(建库):onupgradeneeded 先于 onsuccess
          var vtx = new IDBTransaction(db0, [], "versionchange");
          db0.__vcTx = vtx;
          var uev = { type: "upgradeneeded", target: req, oldVersion: 0, newVersion: version || 1 };
          req.result = db0;
          if (req.onupgradeneeded) __lsmIDBFire([req.onupgradeneeded], uev);
          __lsmIDBFire(req.__handlers["upgradeneeded"] || [], uev);
          req.__succeedDb(db0);
          return;
        }
        var v = version || 1;
        if (v > meta.version) {
          var oldV = meta.version;
          meta.version = v;
          var db1 = new IDBDatabase(meta);
          var vtx1 = new IDBTransaction(db1, [], "versionchange");
          db1.__vcTx = vtx1;
          var uev1 = { type: "upgradeneeded", target: req, oldVersion: oldV, newVersion: v };
          req.result = db1;
          if (req.onupgradeneeded) __lsmIDBFire([req.onupgradeneeded], uev1);
          req.__succeedDb(db1);
          return;
        }
        if (v < meta.version) {
          req.__fail(__lsmIDBErr("VersionError", "请求版本低于现有版本"));
          return;
        }
        req.__succeedDb(new IDBDatabase(meta));
      } catch (e) {
        req.__fail(e);
      }
    }, 0);
    return req;
  },
  deleteDatabase: function (name) {
    var req = new IDBRequest();
    req.source = this;
    setTimeout(function () {
      delete __lsmIDBDbs[name];
      req.__succeed(undefined);
    }, 0);
    return req;
  },
  databases: function () {
    return new Promise(function (resolve) {
      setTimeout(function () {
        var out = [];
        var names = Object.keys(__lsmIDBDbs);
        for (var i = 0; i < names.length; i++) out.push({ name: names[i], version: __lsmIDBDbs[names[i]].version });
        resolve(out);
      }, 0);
    });
  }
};
// onupgradeneeded 也走 addEventListener(upgradeneeded)
(function () {
  var origAdd = IDBRequest.prototype.addEventListener;
  IDBRequest.prototype.addEventListener = function (type, fn) {
    if (type === "upgradeneeded") {
      if (!this.__handlers["upgradeneeded"]) this.__handlers["upgradeneeded"] = [];
      this.__handlers["upgradeneeded"].push(fn);
      return;
    }
    origAdd.call(this, type, fn);
  };
})();
// open 路径的成功派发:upgradeneeded 同步触发后,success 再异步一拍
IDBRequest.prototype.__succeedDb = function (db) {
  var self = this;
  this.result = db;
  this.readyState = "done";
  setTimeout(function () { self.__dispatch("success"); }, 0);
};

// 挂载:全局 + __lsmWin(站点 polyfill 已装则不覆盖)
(function () {
  var G = typeof globalThis !== "undefined" ? globalThis : window;
  if (!G.indexedDB) {
    G.indexedDB = __lsmIndexedDB;
    G.IDBKeyRange = IDBKeyRange;
    G.IDBFactory = function IDBFactory() {};
    G.IDBDatabase = IDBDatabase;
    G.IDBTransaction = IDBTransaction;
    G.IDBObjectStore = IDBObjectStore;
    G.IDBIndex = IDBIndex;
    G.IDBRequest = IDBRequest;
    G.IDBCursor = IDBCursor;
    try {
      if (typeof DOMException === "undefined") {
        G.DOMException = function DOMException(msg, name) {
          this.message = msg || "";
          this.name = name || "Error";
          this.code = 0;
        };
      }
    } catch (e) {}
  }
  try {
    if (typeof __lsmWin !== "undefined" && __lsmWin && !__lsmWin.indexedDB) {
      __lsmWin.indexedDB = __lsmIndexedDB;
      __lsmWin.IDBKeyRange = IDBKeyRange;
    }
  } catch (e) {}
})();
"##;
