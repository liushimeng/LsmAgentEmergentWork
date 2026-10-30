# 45 - 京东首屏 jQuery 链断裂与查看器死亡五根因(2026-10-30 实测)

> 场景:`MCP_Web_LsmAgentBrowser_Use` 打开 `https://www.jd.com`(1 万元装机比价任务,
> headed 模式,要求人工登录后搜索/看评论)。现象:首页只剩服务器静态壳
> 「显示不全、布局混乱」→ 风控弹「京东安全验证」滑块 → 降级壳;登录环节查看器
> 窗口消失后 `request_human` 提示「隐藏模式人工无法操作」,任务卡死。
> 根因定位方法与修复全部落地,京东 14 个官方外链脚本离线复现全链通过。

## 故障链全景

```
DocumentFragment 裸对象(无 cloneNode/childNodes)
  └→ jQuery 1.6.4 support 探测 `l.cloneNode(!0).cloneNode(!0).lastChild.checked`
       当场 "not a callable function" → jQuery 整体装载失败
            ├→ 首页全部动态内容(登录栏/轮播/楼层)不渲染 = 「显示不全」
            └→ wl.js(京东风控指纹,依赖 jQuery)连锁死亡
                 └→ 京东判定异常客户端 → 安全验证滑块 → 降级壳(风控表现)
查看器窗口被人工误关
  └→ load_locked 只调 push_frame(死句柄 no-op,永不重生)
       └→ Agent navigate 到 passport 登录页成功,但窗口回不来
            └→ request_human 只能提示「隐藏模式」→ 登录物理不可行 → 任务卡死
```

**显示问题与风控问题是同一根因的两层**:wl.js 采集不到指纹信号,京东就把客户端
按异常处理。修 jQuery 链 = 同时改善两者。

## 五根因与修复

### 1. DocumentFragment 裸对象(显示不全主犯)

- 探测原句:`l=c.createDocumentFragment(), l.appendChild(a.firstChild),
  k.checkClone=l.cloneNode(!0).cloneNode(!0).lastChild.checked`。
- 旧实现只返回 `{__kids, appendChild}`;`cloneNode/firstChild/lastChild/childNodes` 全缺。
- 修复:`__mkFragment()` 基于 `__newEl("#document-fragment")` 全接口建,
  `__fragify` 逐 clone 重贴 `nodeType=11`(cloneNode 经 `__newEl(t)` 重建会回落成 1)。

### 2. __newEl 动态节点缺 matches 家族

- Sizzle 收编**快照元素**的 `matchesSelector`(`a.matchesSelector||a.mozMatches…`),
  再 `b.call(c.createElement("div"),"div")` 探测 —— `this` 落到动态节点,
  `this.matches` 为 undefined,同样 "not a callable function"。
- 修复:`__newEl` 补 `matches/matchesSelector/closest`(closest 沿 `__parentDyn` 上溯)。

### 3. navigator.javaEnabled 缺失(wl.js 指纹断链)

- wl.js `_e`:`e.javaEnabled=t&&t.javaEnabled()?1:0` —— 不带 typeof 守卫直接调用。
- 修复:navigator 补 `javaEnabled(){return false}`(现代 Chrome 恒 false)。

### 4. RegExp.$1 legacy 静态属性(js_security_v3 断链)

- js_security_v3 老式日期格式化:`/(y+)/i.test(s)` → `RegExp.$1.length`。
- Boa 0.22 未实现 `RegExp.$1`(读出 undefined);`undefined.length` 抛
  "cannot convert 'null' or 'undefined' to object"。
- ⚠️ **Boa 文案歧义复发**:该文案与「读 undefined 属性」共用,勿按 ToObject 误判方向。
- 修复:`__lsmRegExpStaticsInstall()` 包装 `exec/test/match` 三入口记录 `$1..$9`/`$_`,
  `__lsmRefresh` 每轮重装(replace 回调等入口保留旧值,近似)。

### 5. 查看器死亡后永不重生(登录卡死主犯)

- `load_locked`(navigate 路径)只 `push_frame` —— 对死句柄 no-op;只有 `open` 走 `ensure_view`。
- 修复 1:`load_locked` 改 `ensure_view(tab)`(幂等:死句柄重建,活句柄等价推帧)。
- 修复 2:`LsmBrowserManager::revive_viewer()` 公共 API;
  `request_human` 在 `page_visible=false` 时先复活,复活成功弹窗照常指向窗口,
  只有真·无头页才走隐藏模式提示。

## 排查方法沉淀(复用价值)

1. **真实源码离线复现**:curl 拉真站 HTML + 全部外链脚本,`JsHandle::spawn() +
   RunScripts` 逐脚本/逐前缀二分 —— 毫秒~分钟级定位,不碰真站不被风控。
2. **列号陷阱**:`for_in_guard` 源码改写(`EXPR||{}`)令堆栈列号漂移,定位前先
   `LAEW_JS_FORIN_GUARD=off`,真凶列号才对得上(本次 18743→18647)。
3. **wrapper 捕栈**:`try{(function(){SRC})();}catch(e){window.__err=String(e)+'||'+e.stack}`
   —— 管线日志只留消息行,栈要在 JS 侧捕获回传。
4. **"not a callable function" 系列速查**:fragment 裸对象/动态节点缺 matches/
   navigator 缺 javaEnabled,全在 jQuery 家族探测路径上,先查本篇。
5. **慢≠死**:pc-tk.js(50KB 混淆签名 SDK)单脚本 Boa 实测 **191s**,首屏全链 222s;
   「线程预算耗尽」可能只是慢,`request_with_budget` 放宽再判死。管线预算默认
   已从 120s 提到 **240s**(`PIPELINE_BUDGET_MS`)。
6. **HITL 弹窗归属鉴别**:`/var/folders/.../laew_human_ui_payload_<pid>_<seq>.json`
   文件名即进程 PID+请求序号;并行多 laew 会话时先对 payload 判断弹窗归属,
   别把别人的弹窗当自己的(本次实测踩坑)。

## 验证口径

- 离线:京东真实首页 HTML + 14 官方外链脚本,修复前 jQuery/`wl`/`js_security` 全断;
  修复后全部装载(jQuery 1.6.4 就位、`jQuery('#key')` 命中搜索框、`jQuery('*')` 637 元素)。
- 单测:`script/tests_jd_compat.rs` 六条锁死(含 jQuery support 块真实源码形态的最小同构)。
- 真机:登录/搜索链路下一轮 `MCP_Web_LsmAgentBrowser_Use_jd_02` 验证(本轮风控已弹过
  验证,京东 IP 级频控见记忆:~117 次 search/70 分钟触发,横幅「访问频繁」形态)。
