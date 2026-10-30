# 天猫首屏骨架、登录入口缺失与 Fetch/Console 初始化断链

## 背景

2026-10-30 天猫组装机真机任务（`llaew_20261030_145949.log`、`llaew_20261030_154540.log`）：

- 初始首页只有灰色骨架，`input`、搜索入口、登录入口均为 0，无法执行用户要求的“先人工登录”。
- 修复主应用断点后，搜索框、分类和商品节点开始挂载；但页头用户区 `.tbh-user.J_Module` 仍为空，登录入口缺失。
- 本文只记录通用运行时与页面初始化根因，不把问题笼统归为“天猫反爬”。本轮未出现 baxia punish 滑块。

## 根因链

### 1. React 主应用：`Request` 构造器缺失

现象：HTTP 200、标题和 SSR SEO 文案存在，但首屏无控件，`page_health.script_budget_cut=true`。

`inspect(console)` 的首个主应用异常为：

```text
Unhandled promise rejection ReferenceError Request is not defined
```

异常栈进入 `initialize` 和 React render。内核原先只有 `fetch(url, opts)`，没有标准 `Request`，所以 React 主应用初始化直接中断。

### 2. React Router：`Request.signal` 缺失

补上构造器后，同一初始化链继续推进，但出现：

```text
TypeError: cannot convert 'null' or 'undefined' to object
```

按 bundle 异常列号取回 `framework.js` 源码，实际调用为：

```js
m = new Request(m.url, { signal: m.signal });
// 后续 loader 包装：
t.signal.addEventListener("abort", f);
```

只有 `url/method/headers/body` 而没有 `signal` 的 Request 仍不符合 React Router 依赖的最小标准契约。

### 3. tbnav 页头：`console.clear` 缺失

主应用挂载后，页头用户模块仍为空的独立原因是 `o.alicdn.com/tbhome/tbnav/index.js` 抛 TypeError。异常函数 `Hl` 的源码为：

```js
function Hl(e) {
  e.clearOnInit && setTimeout(console.clear.bind(console), 500)
}
```

天猫首页配置 `cleanConsole=true`，因此必然走这里。内核 console 没有 `clear`，读取 `undefined.bind` 时脚本终止，后续 `J_SiteNav` / 用户区初始化不再执行，最终表现为“请登录”入口不存在。

### 4. mtop JSONP：不要再把 HTTP 200 当成脚本已执行

网络台账里三次 mtop 请求均为 200，console 却有动态脚本执行失败。通过同一浏览器内核读取响应正文，确认其确实是 JSONP：

```js
mtopjsonptbpc1({"api":"...","data":{},"ret":["FAIL_SYS_ILLEGAL_ACCESS::非法请求"],...})
```

所以“响应不是 JavaScript”不是根因；需要继续区分回调注册缺失、接口鉴权失败与主应用初始化断链。接口返回的 `data:{}` 也不能被当成商品/用户数据取证成功。

## 通用修复

落点：`src/agent/browser/script/prelude.rs`。

1. 增加 `Headers`：大小写不敏感，支持对象、键值对数组和 Headers 初始化，提供 `get/set/has/append/delete/forEach` 与迭代器。
2. 增加 `Request`：支持 URL/Request 克隆、method 规范化、headers/body、常用选项和 `signal`。
3. `fetch(input, init)` 同时接受字符串与 Request；调用方 init 正确覆盖 Request 的 method/headers/body，响应头也统一为 Headers。
4. 增加可 bind、可注册定时器的 `console.clear`。

这些实现不含天猫域名判断，其他使用 React Router、Fetch 或 console 初始化清理的站点共用。

## 验证

- `tests/web_fetch_request.rs`：真实 mock HTTP 验证 Request 的 method/headers/body 进入请求线、init 覆盖语义、Headers 大小写与 Request.signal。
- `tests/web_console_api.rs`：真实页面执行 `setTimeout(console.clear.bind(console), 500)`，验证初始化链不断。
- `tests/web_use_engine.rs` 原有字符串 URL fetch 用例保持通过。
- Release 真站阶段复测：主应用从 0 个 input 推进到 7 个 input、完整搜索按钮、分类导航与商品/链接节点；搜索选择器 `#q` 可用。
- console.clear 修复后的最终真站结果以本轮最终日志为准；若页头仍缺登录入口，应继续取 console 的**下一首个异常**，不要重复修改已证实的三处。

## 诊断方法

1. `inspect(page_health)` 看 `console_errors/script_pending/script_budget_cut`。
2. `inspect(console)` 找主应用与导航的首个真实异常，过滤 `[chain]` 诊断噪声。
3. `LAEW_DUMP_HTML` 判断 SSR/动态 DOM 是否推进；`LAEW_DUMP_DISPLAY` 区分“DOM 已挂载”和“布局/光栅未产出”。
4. 对压缩 bundle 按异常列号取回邻近源码，避免仅凭 `ToObject` 文案猜根因。
5. 真站探针验证后删除，正式回归统一留在 `tests/` 的 mock 端到端测试中。

## 已知边界

- iframe 内登录/验证码内容不由当前内核渲染；需要顶层导航到 iframe src，或按工具能力提示改用 Chrome。
- mtop 的非法访问/风控响应必须如实呈现，不能伪造登录态或商品数据。
- 首屏大 bundle 与整页重排仍有明显耗时；性能问题应与功能断链分开评估。
