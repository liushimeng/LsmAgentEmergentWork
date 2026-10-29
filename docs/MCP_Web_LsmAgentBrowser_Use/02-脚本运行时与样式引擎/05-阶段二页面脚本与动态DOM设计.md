# 阶段二第一步:页面脚本执行管线与动态 DOM

> 对应总体路线图 Phase 2(`../01-总体设计与架构/01-LsmAgentBrowser总体设计与分阶段路线图.md`)。
> 目标验收:**依赖 JS 渲染的常见站点可读取正文与交互**。
> 状态:✅ 已实现(本文档即实现记录,边界见文末)。
> 后续 C 样式能力见 `06-阶段二第二步CSS级联与外链样式加载设计.md`。

## 一、问题与目标

阶段一的 JS 能力是「快照桥」:每次 `eval_js` 新建一个 Boa Context,把 DOM 序列化成快照注入,
脚本产生的动作(actions)回传宿主执行。三个根本缺陷:

1. **页面自身 `<script>` 完全不执行**——依赖 JS 渲染的站点正文为空;
2. **无会话性**——跨 `eval_js` 调用的全局变量不保留,页面脚本与 Agent 脚本无法共享状态;
3. **DOM 修改不落地**——脚本里的 `innerHTML=...`、`appendChild` 等结构性修改不会反映到布局与后续读取。

本轮引入**每标签页独立 JS 线程 + 动作应用管线**,在不破坏 `Send` 约束与现有契约的前提下补齐以上三点。

## 二、总体架构

```
                    ┌────────────────────────────────────────────┐
 async 侧(宿主)     │ JS 线程(每 Tab 一条,std::thread)           │
                    │                                            │
 load_locked ──────▶│ 持久 boa_engine::Context(全局变量跨调用保留) │
   1. 取主文档        │  ├─ PRELUDE v2:document/window/location…   │
   2. parse DOM      │  ├─ __lsm_refresh(nodes, meta) 注入最新快照  │
   3. 收集脚本        │  ├─ native:__lsmFetchNative / atob / btoa  │
      (inline+外链)  │  └─ 事件循环:微任务 + 虚拟时钟定时器          │
   4. 预取外链脚本    │                                            │
   5. snapshot nodes │   fetch()/XHR ──blocking HTTP──▶ SSRF+TLS  │
        │            │                                            │
        ▼            └───────────────▲────────────────────────────┘
 spawn_blocking: send Eval/RunScripts/Dispatch, recv JsReply
        │                            │
        ▼                            │
 dom_ops::apply(actions) ────────────┘  actions: set_html/append_html/
   kuchikiki DOM 变异                      set_attr/set_text/remove_node/
   → serialize → tab.html                  set_value/set_cookie/navigate…
   → layout::compute → tab.layout
   → console/network 台账增量
```

核心决策:

| 决策 | 理由 |
| --- | --- |
| **每 Tab 一条专用线程持有 Boa Context** | Boa Context 非 `Send`,不能跨 await 保存;线程所有权隔离后,宿主 `Send` 约束零破坏。线程数上限 = `LAEW_WEB_MAX_TABS`(默认 8) |
| **DOM 仍在宿主侧(kuchikiki),按「往返同步」应用动作** | JS 线程持有快照,结构性修改以 action 回传,宿主应用到真实 DOM 后重新序列化回 `tab.html`。所有既有读路径(inspect/elements/layout)零改动 |
| **`fetch`/XHR 为阻塞式原生函数** | JS 线程是普通 `std::thread`,`reqwest::blocking` 可用;`then` 链同步展开,常见 `fetch().then(r=>r.json()).then(render)` 模式直接工作 |
| **定时器走虚拟时钟** | `setTimeout(fn, 5000)` 不真等 5s,事件循环直接快进到到期时刻;防止无限 `setInterval` 拖死页面加载,总回调数/墙钟双上限 |
| **外链脚本宿主侧预取** | 复用异步 `HttpClient`(Cookie Jar、SSRF、TLS、字符集全一致);JS 运行时的 `fetch()` 才走线程内 blocking 客户端 |
| **导航即换线程** | 新页面新全局状态;旧线程发 `Shutdown` 后自然退出 |

## 三、模块划分(`src/agent/browser/script/`)

| 文件 | 职责 |
| --- | --- |
| `mod.rs` | 公共类型:`PageEnv`(owned 页面环境)、`ScriptSource`、`JsRequest`/`JsReply`、`JsHandle`(mpsc::Sender 包装,Clone+Debug);`spawn_for_page()` 建线程;`scripts_enabled()` 开关 |
| `runtime.rs` | 线程主循环:Boa Context 装配(RuntimeLimits 防 while(true) 死循环)、native 函数注册、PRELUDE 求值、每请求处理、事件循环(timers + jobs)、catch_unwind 包裹 |
| `prelude.rs` | PRELUDE v2 JS 源码常量(见下节) |
| `dom_ops.rs` | 宿主侧动作应用:`apply(actions, doc, base_url)`;`collect_scripts(doc, base_url)` 内联脚本提取 + 外链清单;快照父索引支持 |
| `tests.rs` | 单元测试:dom_ops 应用、脚本收集、PRELUDE 行为(fetch/timers/事件/innerHTML) |

### 3.1 消息协议

```rust
pub enum JsRequest {
    RunScripts { scripts: Vec<ScriptSource>, env: PageEnv, nodes: Vec<SnapshotNode>,
                 reply: std::sync::mpsc::Sender<JsReply> },
    Eval      { script: String, env: PageEnv, nodes: Vec<SnapshotNode>,
                 reply: std::sync::mpsc::Sender<JsReply> },
    Dispatch  { selector: String, event: String, env: PageEnv, nodes: Vec<SnapshotNode>,
                 reply: std::sync::mpsc::Sender<JsReply> },
    Shutdown,
}
```

- 宿主调用一律经 `tokio::task::spawn_blocking`(send + `recv_timeout`),不阻塞 reactor;
- `PageEnv` 为 owned 版 `JsPageMeta`(url/title/cookies/local/session/viewport/scroll/ua);
- `JsReply { outcome: JsOutcome, network: Vec<NetRecordLite>, handled: bool, default_prevented: bool }`;
- 超时(默认 = 脚本预算)后视该线程为不可用:置 `tab.js = None`,下轮重建线程。

### 3.2 事件循环(线程内)

```
执行脚本(每个 <script> 一次 eval)
  → dispatch DOMContentLoaded → dispatch load
  → loop {
        ctx.run_jobs()               // 微任务(Promise/await 续体)
        __lsmTick()                  // 虚拟时钟快进 + 执行到期定时器
        无待处理 / 总回调超上限(256)/ 墙钟超预算 → break
    }
  → 收集:outcome.actions / console / network / set_cookie
```

- **RuntimeLimits**:`loop_iteration_limit = 10_000_000`、递归深度 256,防 `while(true){}` 挂死线程;
- 墙钟预算默认 10s,`LAEW_WEB_SCRIPT_BUDGET_MS` 覆盖;
- `LAEW_WEB_SCRIPT=off` 全关:导航不建线程,`eval_js` 回退阶段一快照桥。

## 四、PRELUDE v2(线程内 JS 环境)

阶段一 PRELUDE 是一次性 IIFE;v2 改为**一次性全局装配 + 每请求 refresh**:

```js
// 装配(一次):globalThis.document/window/location/navigator/... + __lsm 内核对象
// 每请求:__lsmRefresh(nodes, meta) —— 原地替换快照数组与页面元信息
//         (已捕获的元素引用会指向旧快照,重新 query 即可;文档化边界)
```

与阶段一的行为差异(全部是增强):

| 能力 | 阶段一 | 阶段二(本轮) |
| --- | --- | --- |
| 全局变量跨 eval_js | ❌ 每次 new Context | ✅ 线程持有持久 Context |
| `innerHTML = html` | 等价 textContent | ✅ `set_html` action → 宿主 kuchikiki 解析 fragment 真注入 |
| `appendChild/insertBefore/removeChild/remove` | no-op | ✅ JS 侧轻量序列化为 HTML → `append_html`/`insert_html`/`remove_node` action |
| `document.write/writeln` | no-op | ✅ 缓冲输出,脚本末尾按序注入(追加语义,简化) |
| `fetch`/XHR | 记录不执行 | ✅ blocking 真请求;响应 `text()/json()` 可用;Set-Cookie 回传宿主 Jar |
| `setTimeout/setInterval/rAF` | 同步执行一次 | ✅ 虚拟时钟事件循环,多轮执行,cancel 可用 |
| `addEventListener` | no-op | ✅ 选择器级注册表;宿主 click/input/change/submit 触发 dispatch |
| `DOMContentLoaded/load` | — | ✅ 管线按序派发;`readyState` 三态推进 |
| `preventDefault` | — | ✅ 事件对象可阻止默认行为,宿主遵从 |
| `atob/btoa` | 自引用 bug | ✅ Rust native(base64) |
| `performance.now` | 恒 0 | ✅ 虚拟时钟毫秒 |
| `style.display='none'` 等赋值 | 丢失 | ✅ Proxy 拦截合并为 `set_attr style`,参与可见性判断 |
| `el.click()` | action 回传 | ✅ 同左 + 若有注册 listener 立即同步触发 |

**fetch 原生桥**:`__lsmFetchNative(url, optsJson) -> jsonString`,返回
`{ok,status,statusText,url,headers:{},body,redirectChain}` 或 `{error}`。JS 侧包装成 Response 对象
(`text()/json()/headers.get()`),`fetch()` 返回 thenable,`XMLHttpRequest` 走同一桥。

**事件对象**:`__mkEvent(type)` 提供 `type/target/currentTarget/preventDefault()/stopPropagation()/defaultPrevented`,
dispatch 时 `target` 指向快照元素。

## 五、宿主侧管线

### 5.1 导航(`load_locked` / `new_page`)

```
取主文档 → Document::parse → collect_scripts:
    inline: <script> 无 src 的文本(≤ 256KB/个,≤ 32 个)
    external: <script src>(http/https,≤ 16 个)→ 宿主预取(每脚本 ≤ 2MB)
多轮执行(≤ 3 轮):
    snapshot_nodes(含新增 parent 索引) → RunScripts → apply(actions)
    → 重新序列化 → 若出现新 <script> 继续下一轮
终态:tab.html = 序列化后 DOM;layout 重算;console/network 台账追加;title 以脚本改写后为准
```

### 5.2 动作应用(`dom_ops.rs`)

| action | DOM 应用 |
| --- | --- |
| `set_attr` / `remove_attr` | 节点属性写删(class/style/value 通用) |
| `set_text` | 清空子树,置文本节点 |
| `set_html` | 清空子树,`parse_fragment` 注入 |
| `append_html` / `insert_html`(before) | 对应位置 fragment 注入 |
| `remove_node` | `detach()` |
| `set_value` / `set_checked` | `value`/`checked` 属性 + `TabState.form_values` |
| `set_title` / `set_cookie` / `storage_*` / `navigate` / `new_tab` / `submit` / `scroll` | 沿用阶段一宿主处理 |

选择器定位:action 携带 `document::stable_selector` 产物;`nth` 兜底。

### 5.3 交互事件桥(`api.rs`)

- **click**:计算默认 NavPlan 后,若 `tab.js` 存在 → `Dispatch{selector, "click"}` →
  应用返回 actions;若 `default_prevented` 或产生 `navigate/submit/new_tab` 动作则按动作执行,
  否则继续默认 NavPlan(贴近真实浏览器:handler 与默认行为并存)。
- **input_text / human_input**:写值后 `Dispatch{selector, "input"}`,再 `change`;
- **select_option / select_dropdown**:写值后 `Dispatch{selector, "change"}`;
- **key_press Enter 提交表单前**:`Dispatch{form_selector, "submit"}`,`preventDefault` 可拦截。

### 5.4 eval_js

走线程 `Eval`(持久全局);返回值语义与阶段一一致(`value/value_type/error/console`)。
actions 应用后同样回写 `tab.html` 与布局——**脚本 DOM 修改立即反映到后续 inspect/elements/截图**。

## 六、安全与资源

| 项 | 策略 |
| --- | --- |
| SSRF | 线程内 fetch 与宿主预取共用 `url_safety::is_safe_endpoint`;`LAEW_ALLOW_PRIVATE_ENDPOINT` 放行 |
| TLS | 与主网络栈一致:`LAEW_TLS_INSECURE` / IP 主机放宽 |
| 死循环 | Boa RuntimeLimits(loop/递归)+ 墙钟预算 + 定时器回调总数上限 |
| 线程泄漏 | 宿主 `recv_timeout` 超时后弃用句柄重建线程;`Shutdown`/关页/导航均触发退出 |
| 体积 | 单脚本 256KB、外链 2MB、外链数 16、inline 数 32、actions 单轮 4K、console 单轮 2K |
| panic | 线程主循环 `catch_unwind`,失败转为该次 `JsReply` 错误,不波及进程 |

## 七、环境变量

| 变量 | 取值 | 行为 |
| --- | ---- | ---- |
| `LAEW_WEB_SCRIPT` | `off/0/false/no` | 关闭页面脚本执行与 JS 线程;`eval_js` 回退阶段一快照桥 |
| `LAEW_WEB_SCRIPT_BUDGET_MS` | 正整数(默认 `10000`) | 单次脚本管线(导航/eval/dispatch)墙钟预算 |

## 八、验证

| 层 | 内容 |
| --- | --- |
| 单元(`script/tests.rs`) | dom_ops 各 action 应用;collect_scripts 提取;PRELUDE:innerHTML 注入、fetch mock 桥、timers、事件 dispatch、全局保留 |
| 集成(`tests/web_use_engine.rs`) | mock HTTP 站点:inline 脚本渲染正文、外链脚本、fetch 数据渲染、setTimeout 延迟渲染、click listener 阻止默认导航、eval_js 全局跨调用保留 |
| 端到端 | `./rebuild_restart_app.sh` 后真实二进制 `-p` 冒烟 |

## 九、本轮边界(阶段二第二步待办)

1. ~~**CSS 级联/继承/媒体查询完善**~~ → 已在阶段二第二步落地(见 `06-阶段二第二步CSS级联与外链样式加载设计.md`);
   动画/过渡/伪元素与 `@import` 递归仍待后续阶段;
2. **MutationObserver / IntersectionObserver** —— 未实现;
3. **Shadow DOM / Web Components** —— 阶段五;
4. **async/await 深度语义**:`await` 依赖 Boa 微任务队列,本轮经 `run_jobs` 支持已解析 Promise 的续体;
   永不 resolve 的 Promise 会导致该轮静默截断(不挂死);
5. **`document.write` 为追加语义**,不模拟真实流式解析插入点;
6. **页面脚本发出的 `fetch` 不经过宿主 Cookie Jar 实时同步**(取会话开始时的快照,Set-Cookie 回传合并),
   极少数依赖即时 Cookie 读写的站点可能行为偏差;
7. **捕获的 DOM 引用跨 refresh 指向旧快照**(全局变量保留,元素引用需重新 query);
8. **V8 后端** 仍为可选特性,默认 Boa。
