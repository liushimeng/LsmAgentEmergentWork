# 22 - IndexedDB 最小内存实现与闲鱼动态子树渲染中断根因

- 日期:2026-11-20
- 场景:`MCP_Web_LsmAgentBrowser_Use_goofish_02` 真机(闲鱼 goofish.com 全品类调研任务)
- 代码:`src/agent/browser/script/idb_js.rs`(实现)、`tests_idb.rs`(单测)、`src/agent/tools/mcp_web_lsm_browser_use/mod.rs::merge_open_params`(附带修复)

## 根因链(真机实测证据)

```
window.indexedDB 不存在(内核零实现)
  → 闲鱼动态 chunk(webpack 按需加载,eval 执行)探测后 throw
    Error('该浏览器不支持indexedDB') —— 16~18 条 Unhandled promise rejection
  → 连锁 TypeError: cannot convert 'null' or 'undefined' to object
  → MessagePort onmessage 失败(onmessage 回调内抛上述 TypeError)
  → React 动态子树渲染中断:
      首页 feeds-main 容器 → 空态「小闲鱼暂时没有推荐～」
      搜索页 fish_card 3 张全空文本 + 19 个 ant 骨架不收敛
      gaia index.get 接口循环重试 20 次(缓存写入失败→重试)
```

**关键排除项**(排查时别再走弯路):mtop feed 接口 200 + 206KB 数据正常返回(网络/Cookie/风控链路全部正常);静态 SSR 内容(导航/热搜词/页脚)渲染正常(渲染引擎无故障);非 iframe 遮挡、非视口、非多标签。

**主 bundle 侧证据**:`p_layout.js` 对 indexedDB 只做 `!!window.indexedDB`(设备指纹信号);抛错文本来自动态 chunk 的本地缓存 SDK(阿里系惯用 `if (!window.indexedDB) throw ...`)。

## 修复设计

### IndexedDB 最小内存实现(PRELUDE 注入件)

- **位置**:`script/idb_js.rs` 导出 `IDB_JS`,拼进 `PRELUDE`(`PRELUDE_BODY + CANVAS_JS + SELECTOR_JS + GLOBALS_JS + IDB_JS` 尾部)。
- **存储**:`__lsmIDBDbs` PRELUDE 全局,随 JS 线程存活 —— 跨导航持久、进程退出即失(与 Cookie/localStorage 内存定位一致;站点侧表现为缓存 miss 重新拉接口,无功能断链)。
- **异步**:onsuccess/onerror/oncomplete/onupgradeneeded 全部经 `setTimeout(fn, 0)` 排队(虚拟时钟),与真实浏览器异步事件语义一致;站点脚本「先 open 后赋回调」的时序天然安全。
- **fail-soft 铁律**:实现内部异常一律走 `request.onerror` + `__cerror` console 记账,**永不把异常 throw 回站点初始化链**(补标准 API 的既有经验:站点 SDK 只做存在性探测就直接用)。
- **API 面**:IDBFactory(`open/deleteDatabase/databases/cmp`)、IDBDatabase(`createObjectStore/deleteObjectStore/transaction/close/objectStoreNames`)、IDBTransaction(`objectStore/abort/oncomplete`)、IDBObjectStore(`get/getKey/getAll/getAllKeys/put/add/delete/clear/count/index/createIndex/openCursor`)、IDBIndex(`get/getKey/getAll/getAllKeys/count`)、IDBCursor(`continue/advance/update/delete`)、IDBKeyRange(`only/bound/lowerBound/upperBound/includes`)、轻量 `DOMException`。
- **键语义**:数字/字符串/Date/复合数组键统一归一化进可排序字符串空间(数字按数值序);keyPath 支持嵌套 `a.b` 与数组;`autoIncrement` 自增;put 的 inline key 回写 value。
- **版本语义**:建库/升版 → `onupgradeneeded`(oldVersion/newVersion)→ `onsuccess`;同版本直连;**回退 → onerror(VersionError)**。
- **事务生命周期**:请求计数 pending,归零后异步 `oncomplete`;空事务也异步完成(`__armEmpty`);complete 后再操作抛 `TransactionInactiveError`(真实语义)。
- **边界(如实上报)**:无 onblocked/多连接仲裁、无磁盘持久化、无 StructuredClone 深拷贝语义(存引用)。

### 附带修复:merge_open_params 静默丢参数

`open` 顶层有 url 时 params 整体被丢弃 → `params.window_width/window_height` 静默失效(响应 `requested` 恒 null)。改为**已知白名单键无条件 or_insert 上提**(顶层显式值优先,未知键仍忽略)。白名单:url/window_width/window_height/mode/new_window/user_agent/wait_until/timeout_ms/reload/force/reuse/page_id/save_path/ocr/full_page。

## 单测锁死(`script/tests_idb.rs`,9 例)

探测面(`!!window.indexedDB`)、open→upgradeneeded→put→get 全链、keyPath 嵌套+外显 key、KeyRange+数字键排序、cursor continue 遍历、同版本直连+版本回退 VersionError、add 冲突 ConstraintError+deleteDatabase、index 二级键查询、addEventListener 双通道+cmp/databases。

## 排查手册(同类「接口 200 但页面空」问题)

1. `inspect(info=console)` 看 first_errors —— 中文错误文本(如「该浏览器不支持indexedDB」)直接定位缺失 API。
2. `inspect(info=network)` 确认数据接口 200 且有 body 字节(排除风控/网络)。
3. `page_health` 的 `visible_text_chars` 与 SSR 静态内容对照:静态在/动态空 → 脚本链断,不是渲染引擎问题。
4. 主 bundle(`p_index.js`/`p_layout.js`)grep 缺失 API 名:只出现在指纹表达式(`!!window.x`)→ 全局对象补存在性即可;有完整调用 → 按本文件模式补语义实现。
5. 重站点动态 chunk 用 eval 执行,堆栈形如 `at <main> (eval at :1:NNNNN)`,拿不到源码时从错误文本反查 SDK 类型。
