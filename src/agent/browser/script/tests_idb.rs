//! IndexedDB 最小内存实现回归单测(2026-11-20 闲鱼真机根因)。
//!
//! 根因:内核缺 `window.indexedDB`,闲鱼本地缓存 SDK 探测后 throw
//! `该浏览器不支持indexedDB` 且异常未被 catch,React 动态子树(首页信息流/
//! 搜索商品卡)渲染中断 —— mtop feed 接口 200+206KB 数据在手也画不出来。
//!
//! 本文件锁死:探测面(指纹探测 `!!window.indexedDB` 必须过)、
//! open→upgradeneeded→createObjectStore→put→get 全链、keyPath 嵌套提取、
//! getAll/IDBKeyRange 过滤、cursor 遍历、同版本二次 open 直连、
//! 版本回退 onerror(VersionError)、add 冲突 ConstraintError、
//! transaction oncomplete、deleteDatabase。回调经 setTimeout(0),
//! 测试侧 `__lsmTick()` 推进虚拟时钟。

use super::tests::{eval_on, run_scripts};
use super::JsHandle;

const PAGE: &str = "<html><head></head><body></body></html>";

fn eval_str(handle: &JsHandle, script: &str) -> String {
    let reply = eval_on(handle, script, PAGE, "https://a.test/");
    assert!(
        reply.outcome.ok,
        "求值失败: {:?} — 脚本前 120 字: {}",
        reply.outcome.error,
        &script.chars().take(120).collect::<String>()
    );
    reply.outcome.value
}

/// 多次 tick 把 setTimeout(0) 链条(open→upgradeneeded→success→complete)全部冲完。
fn drain(handle: &JsHandle) {
    for _ in 0..12 {
        eval_str(handle, "__lsmTick()");
    }
}

/// 场景一(真机主根因):站点指纹级探测必须通过。
/// `!!window.indexedDB` 是闲鱼 p_layout 设备指纹与缓存 SDK 的共同探测点。
#[test]
fn indexeddb探测面_全局与window均可命中() {
    let handle = JsHandle::spawn();
    let out = eval_str(
        &handle,
        r#"String(typeof indexedDB !== "undefined") + "," + String(!!window.indexedDB) + "," + String(typeof IDBKeyRange !== "undefined")"#,
    );
    assert_eq!(out, "true,true,true", "indexedDB/IDBKeyRange 必须全局可见");
    handle.shutdown();
}

/// 场景二:open→upgradeneeded(建库建表)→onsuccess→put→get 全链往返。
#[test]
fn open建库建表_put_get往返() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
            window.__log = [];
            var req = indexedDB.open("testdb", 1);
            req.onupgradeneeded = function (e) {
              window.__log.push("up:" + e.oldVersion + ">" + e.newVersion);
              var db = e.target.result;
              if (!db.objectStoreNames.contains("cache")) {
                db.createObjectStore("cache", { keyPath: "k" });
              }
            };
            req.onsuccess = function (e) {
              var db = e.target.result;
              window.__dbVersion = db.version;
              var tx = db.transaction("cache", "readwrite");
              var store = tx.objectStore("cache");
              store.put({ k: "name", v: "闲鱼" });
              store.put({ k: "price", v: 999 });
              tx.oncomplete = function () {
                var tx2 = db.transaction("cache", "readonly");
                tx2.objectStore("cache").get("name").onsuccess = function (e2) {
                  window.__got = e2.target.result.v;
                };
                tx2.oncomplete = function () { window.__log.push("done"); };
              };
            };
            req.onerror = function () { window.__log.push("err"); };
        "#],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "页面脚本不得抛错:{:?}", reply.outcome.error);
    drain(&handle);
    let log = eval_str(&handle, "window.__log.join('|') + ';' + String(window.__dbVersion) + ';' + String(window.__got)");
    assert_eq!(log, "up:0>1|done;1;闲鱼", "建库→写→读全链必须打通,实际:{log}");
    handle.shutdown();
}

/// 场景三:keyPath 嵌套提取("a.b")+ 外显 key put + getAll。
#[test]
fn keypath嵌套与外显key_getall往返() {
    let handle = JsHandle::spawn();
    run_scripts(
        &handle,
        vec![r#"
            window.__out = "pending";
            var req = indexedDB.open("nest", 1);
            req.onupgradeneeded = function (e) {
              e.target.result.createObjectStore("s1", { keyPath: "a.b" });
              e.target.result.createObjectStore("s2");
            };
            req.onsuccess = function (e) {
              var db = e.target.result;
              var tx = db.transaction(["s1", "s2"], "readwrite");
              tx.objectStore("s1").put({ a: { b: 7 }, name: "x" });
              tx.objectStore("s2").put("裸值", 42);
              tx.oncomplete = function () {
                var tx2 = db.transaction(["s1", "s2"], "readonly");
                tx2.objectStore("s1").get(7).onsuccess = function (g1) {
                  tx2.objectStore("s2").getAll().onsuccess = function (g2) {
                    window.__out = g1.target.result.name + "," + g2.target.result.join(",");
                  };
                };
              };
            };
        "#],
        PAGE,
        "https://a.test/",
    );
    drain(&handle);
    let out = eval_str(&handle, "String(window.__out)");
    assert_eq!(out, "x,裸值", "嵌套 keyPath 与外显 key 必须均可往返,实际:{out}");
    handle.shutdown();
}

/// 场景四:IDBKeyRange 过滤(bound/lowerBound)+ 排序语义(数字键按数值序)。
#[test]
fn keyrange过滤与数字键排序() {
    let handle = JsHandle::spawn();
    run_scripts(
        &handle,
        vec![r#"
            window.__out = "pending";
            var req = indexedDB.open("kr", 1);
            req.onupgradeneeded = function (e) { e.target.result.createObjectStore("n"); };
            req.onsuccess = function (e) {
              var db = e.target.result;
              var tx = db.transaction("n", "readwrite");
              var st = tx.objectStore("n");
              [30, 4, 200, 7].forEach(function (n) { st.put("v" + n, n); });
              tx.oncomplete = function () {
                var tx2 = db.transaction("n", "readonly");
                tx2.objectStore("n").getAll(IDBKeyRange.bound(5, 100)).onsuccess = function (g) {
                  window.__out = g.target.result.join(",");
                };
              };
            };
        "#],
        PAGE,
        "https://a.test/",
    );
    drain(&handle);
    let out = eval_str(&handle, "String(window.__out)");
    assert_eq!(out, "v7,v30", "数字键须按数值序且 KeyRange 过滤正确,实际:{out}");
    handle.shutdown();
}

/// 场景五:openCursor 遍历全表(continue 推进)。
#[test]
fn cursor遍历_continue推进() {
    let handle = JsHandle::spawn();
    run_scripts(
        &handle,
        vec![r#"
            window.__acc = [];
            var req = indexedDB.open("cur", 1);
            req.onupgradeneeded = function (e) { e.target.result.createObjectStore("c"); };
            req.onsuccess = function (e) {
              var db = e.target.result;
              var tx = db.transaction("c", "readwrite");
              // 无 keyPath 的 store:put 必须带外显 key(IDB 语义,DataError 硬门)
              ["a", "b", "c", "z"].forEach(function (k, i) { tx.objectStore("c").put({ id: k, n: i }, k); });
              tx.oncomplete = function () {
                db.transaction("c", "readonly").objectStore("c").openCursor().onsuccess = function (ev) {
                  var cur = ev.target.result;
                  if (cur) { window.__acc.push(cur.value.id); cur.continue(); }
                };
              };
            };
        "#],
        PAGE,
        "https://a.test/",
    );
    drain(&handle);
    let out = eval_str(&handle, "window.__acc.join('')");
    assert_eq!(out, "abcz", "cursor 必须按序遍历全表,实际:{out}");
    handle.shutdown();
}

/// 场景六:同版本二次 open 直连 success(不再 upgradeneeded);
/// 版本回退触发 onerror(VersionError)。
#[test]
fn 二次open直连_版本回退报versionerror() {
    let handle = JsHandle::spawn();
    run_scripts(
        &handle,
        vec![r#"
            window.__events = [];
            var req = indexedDB.open("ver", 2);
            req.onupgradeneeded = function () { window.__events.push("up"); };
            req.onsuccess = function (e) { e.target.result.close(); window.__events.push("ok"); };
        "#],
        PAGE,
        "https://a.test/",
    );
    drain(&handle);
    // 同版本二次打开:只有 ok,没有第二次 up
    run_scripts(
        &handle,
        vec![r#"
            var req2 = indexedDB.open("ver", 2);
            req2.onupgradeneeded = function () { window.__events.push("up2"); };
            req2.onsuccess = function (e) { e.target.result.close(); window.__events.push("ok2"); };
            // 版本回退:必须 onerror 且 error.name=VersionError
            var req3 = indexedDB.open("ver", 1);
            req3.onerror = function (e) { window.__events.push("err:" + e.target.error.name); };
            req3.onsuccess = function () { window.__events.push("BAD-success"); };
        "#],
        PAGE,
        "https://a.test/",
    );
    drain(&handle);
    let out = eval_str(&handle, "window.__events.join('|')");
    assert_eq!(
        out, "up|ok|ok2|err:VersionError",
        "版本语义:同版本直连、回退报错,实际:{out}"
    );
    handle.shutdown();
}

/// 场景七:add 键冲突 → ConstraintError;deleteDatabase 后重开为空库。
#[test]
fn add冲突与delete_database() {
    let handle = JsHandle::spawn();
    run_scripts(
        &handle,
        vec![r#"
            window.__log = [];
            var req = indexedDB.open("dup", 1);
            req.onupgradeneeded = function (e) { e.target.result.createObjectStore("d"); };
            req.onsuccess = function (e) {
              var db = e.target.result;
              var tx = db.transaction("d", "readwrite");
              tx.objectStore("d").add("one", 1);
              tx.objectStore("d").add("conflict", 1).onsuccess = function () { window.__log.push("BAD"); };
              tx.oncomplete = function () { db.close(); };
            };
        "#],
        PAGE,
        "https://a.test/",
    );
    drain(&handle);
    run_scripts(
        &handle,
        vec![r#"
            var dr = indexedDB.deleteDatabase("dup");
            dr.onsuccess = function () {
              var req2 = indexedDB.open("dup", 1);
              req2.onupgradeneeded = function (e) { e.target.result.createObjectStore("d"); window.__log.push("reup"); };
              req2.onsuccess = function (e2) {
                var db2 = e2.target.result;
                db2.transaction("d", "readonly").objectStore("d").count().onsuccess = function (c) {
                  window.__log.push("count:" + c.target.result);
                };
              };
            };
        "#],
        PAGE,
        "https://a.test/",
    );
    drain(&handle);
    let out = eval_str(&handle, "window.__log.join('|')");
    assert_eq!(out, "reup|count:0", "add 冲突被拒+deleteDatabase 清库,实际:{out}");
    handle.shutdown();
}

/// 场景八:index(createIndex + index.get)二级键查询。
#[test]
fn createindex与index查询() {
    let handle = JsHandle::spawn();
    run_scripts(
        &handle,
        vec![r#"
            window.__out = "pending";
            var req = indexedDB.open("idx", 1);
            req.onupgradeneeded = function (e) {
              var st = e.target.result.createObjectStore("items", { keyPath: "id" });
              st.createIndex("byName", "name");
            };
            req.onsuccess = function (e) {
              var db = e.target.result;
              var tx = db.transaction("items", "readwrite");
              tx.objectStore("items").put({ id: 1, name: "phone", price: 900 });
              tx.objectStore("items").put({ id: 2, name: "bike", price: 800 });
              tx.oncomplete = function () {
                db.transaction("items", "readonly").objectStore("items").index("byName").get("phone").onsuccess = function (g) {
                  window.__out = g.target.result ? ("price:" + g.target.result.price) : "none";
                };
              };
            };
        "#],
        PAGE,
        "https://a.test/",
    );
    drain(&handle);
    let out = eval_str(&handle, "String(window.__out)");
    assert_eq!(out, "price:900", "index 二级键查询必须可用,实际:{out}");
    handle.shutdown();
}

/// 场景九:SDK 惯用 Promise 包装层(requesttoPromise)兼容 —— addEventListener
/// 与 onsuccess 双通道、databases()/cmp() 探测面。
#[test]
fn sdk包装层_addEventListener与cmp_databases() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
            window.__log = [];
            var req = indexedDB.open("sdk", 1);
            req.addEventListener("upgradeneeded", function () {
              req.target.result.createObjectStore("kv");
            });
            req.addEventListener("success", function (e) {
              var db = e.target.result;
              var tx = db.transaction("kv", "readwrite");
              tx.objectStore("kv").put("v", "k");
              tx.addEventListener("complete", function () { db.close(); });
            });
            window.__log.push("cmp:" + indexedDB.cmp(1, 2) + indexedDB.cmp(2, 2) + indexedDB.cmp(3, 2));
            indexedDB.databases().then(function (dbs) {
              window.__log.push("dbs:" + dbs.length);
            });
        "#],
        PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "页面脚本不得抛错:{:?}", reply.outcome.error);
    drain(&handle);
    let out = eval_str(&handle, "window.__log.join('|')");
    assert!(out.starts_with("cmp:-101|"), "cmp 探测面必须返回 -1/0/1,实际:{out}");
    assert!(out.contains("dbs:"), "databases() Promise 必须结算,实际:{out}");
    handle.shutdown();
}
