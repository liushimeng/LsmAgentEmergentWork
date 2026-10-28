# 34 · zhipin 安全验证页构建链四根因与布局三根因修复

> 2026-10-28 真机测试 `Dir_LsmAgentBrowser_zhipin_01`(laew --debug -f,LAEW_BROWSER_MODE=headed)。
> 现象一:验证页滑块永不出现,用户面对空白页只能在 HITL 弹窗点「取消任务」;
> 现象二(用户报障):零星几个文字、布局乱、渲染显示不全。
> 两组根因互相独立:构建链四根因(JS 层)+ 布局三根因(布局引擎层)。

## 一、验证页滑块构建链(修复前逐环静默断裂)

```
getredirect XHR → setInterval 轮询 #rowCode → dynamicLoadJs(captcha-sdk)「先设 src 后 appendChild」
  → onload → Promise → new CaptchaVerify → m() XHR gettype(极验/点选)
  → nextStep(await 微任务链) → v() 装载 gt.0.5.0.js → initGeetest
  → JSONP api.geetest.com/gettype.php → fullpage.9.2.0 主包 → 回调链(unescape) → 滑块注入 #rowCode
```

### 根因 1:动态脚本装载信号序颠倒(「先设 src 后 appendChild」模式 onload 永不触发)

`dynamicLoadJs` 是 zhipin/无数老站点通用装载序:`o.onload=…; o.src=e; appendChild(o)`。
内核 script 元素的 `src` 属性 setter(为 webpack「先 append 后设 src」设计)在 src 赋值这一刻
**同步取回+执行+fire**——此刻元素尚未注册进 `__lsmScriptCbs`(注册在 appendChild),
fire 打在真空(链路追踪实测 `fire captcha-sdk cbs=0`);随后 appendChild 注册时
`__lsmDynLoaded[url]=true` 直接 return 不再补发。**onload 永不触发 → 装载 Promise 永不
resolve → `.then(new CaptchaVerify)` 永不执行 → 整链饿死。**

**修复**(prelude.rs):
- `src` setter 改走 `__lsmRegisterScriptLoad(el)`(注册+装载一体,fire 时回调必在表);
- `__lsmLoadDynScript` 对已装载 URL 按缓存语义**补发信号**:成功→fire load,失败→fire error(不假触发 onload);
- 元素级 `__lsmLoadNotified` 去重:两种装载序都恰好 fire 一次;
- 失败信号只派发 onerror 且**保留注册表**(宿主侧管线外链脚本页面可能再引用)。

### 根因 2:event_loop 排水顺序缺陷(定时器回调排入的微任务永不被排水)

`runtime.rs::event_loop` 原序 `drain_jobs() → __lsmTick() → if pending==0 break`:
定时器回调(200ms 轮询、脚本 onload→Promise 链)排入的微任务在 tick 之后才进队列,
最后一轮定时器后 `pending==0` 直接 break——微任务滞留到**下一次** eval/dispatch 才排水;
而 open 之后的 inspect/screenshot/request_human 全是宿主侧动作不触碰 JS 线程 → 无限期挂起。

**修复**:tick 后追加排水;`pending==0` 时重读 `__lsmTimers.length`(排水后的微任务可能又注册
新定时器),双双静默才退出(受 total_ran/deadline 双上限约束)。

### 根因 3:同步 XHR 派发时序背离浏览器语义(「超时保险丝」必发伪 fail)

内核 XHR send() 同步完成取回并逐档派发 rs(2/3/4)。站点普遍保险丝模式
`xhr.send(); timer=setTimeout(()=>fail("timeOut"),5000)`:rs4 回调里的 `clearTimeout(e.timer)`
执行时 timer 还不存在 → 5 秒后伪 fail 必发(实测 captcha-sdk `m()` 与页面 `HTTP.ajax`
白名单请求先 fail(0) 又伪 fail("timeOut") 双报错,后写 fail 覆盖 success 证据)。

**修复**:send() 取回仍线程内同步,但状态推进与回调派发推迟到微任务(浏览器 send 先返回);
显式 `open(m,u,false)` 保持同步派发;`abort()` 置 `_aborted` 抑制派发。

### 根因 4:缺 Annex B 遗留全局 `escape`/`unescape`(geetest 回调链 ReferenceError)

链路修通后最深处断点:geetest fullpage 的 JSONP 回调链调用 `unescape`——Chrome 至今保留
这对遗留函数,**Boa 未实现** → `ReferenceError: unescape is not defined` 在 eval 内抛出 →
JSONP 整条回调链断(zhipin 页面自身 `getCookie` 也用它)。按 Annex B 语义补齐
(percent 编解码 + `%uXXXX`)。

## 二、布局三根因(用户报障:零星文字/布局乱/渲染不全)

`LAEW_DUMP_DISPLAY` 显示列表一锤定音:`.verify-container` x=28 **w=1824**(应为 520 居中),
整页无居中。离线单测(真站 HTML 直喂 compute)复现 → 纯布局引擎 bug。

| # | 根因 | 修复 |
| --- | --- | --- |
| R1 | column flex 条目交叉宽分支**无视 max-width**(row 分支有,column 缺失)→ `width:100%;max-width:520px` 解成全宽 | 列分支按 CSS 钳序钳制(先 min-width 地板后 max-width 上限) |
| R2 | `justify-content` 在 **min-height 撑起的容器**上 free 恒 0 → 垂直居中永不发生 | `inner_main` 纳入 min-height 地板;`definite_main` 认可 min-height |
| R3 | **`min-width`/`min-height` 属性整体未实现**(无字段无解析)→ `min-height:100vh` 静默丢弃 | 补 ComputedStyle 字段+解析+块级/flex 双侧应用 |

修复后几何:`.verify-container` x=680 w=520 水平居中、卡片 y≈205 视口垂直居中、
title/rowCode 全部收进 440px 内容宽——与真实浏览器一致。

## 三、诊断方法论沉淀(本轮新增设施)

- **`LAEW_JS_CHAIN_TRACE=1`**:动态脚本装载链逐环打点(`[chain] register/load-entry/fire cbs=N`),
  「SDK 已装载但 onload→Promise 链不推进」类断链直接看 cbs 计数;
- **`LAEW_JS_LOOP_TRACE=1`**:JS 线程 event_loop 每轮 pending/ran/任务计数,微任务/定时器滞留可见;
- 排障套路:console 清单 → 链路打点 → 逐环二分(装载/派发/回调),本轮四根因全部此法定位。

## 四、已证伪假设(避免重走弯路)

- ~~微任务异常饿死后续微任务~~:Boa promise reaction 内部隔离异常(单测证实);
- ~~for-in 守卫改写误伤~~:`LAEW_JS_FORIN_GUARD=off` 复测依旧;
- ~~geetest TLS 指纹风控~~:curl 与内核同头均能拿合法 JSONP;「jsonp xss」错误仅因探针用了
  非法 callback 名(geetest 校验 `geetest_\d+` 格式);
- ~~CORS 拦截 JSONP~~:`script:true` 通道免 CORS;探针未 JSON.stringify opts 导致误判
  (顺带发现简单请求 CORS 校验检查了 Allow-Methods——Fetch 规范偏差,留观察)。

## 五、已知残余(如实记档)

- body 背景 `var(--bg-page)` 不上画布(html/body 无盒)——近白色差,影响轻微;
- geetest captchaType=4(图形点选)变体回调链 `UserCallBackError`(厂商内部吞错),比已修的
  type=1(极验滑块)更深一层;交互式验证码由 MCP_Web_Chrome_Use 兜底(双浏览器分工);
- SVG 图标不渲染(内核 SVG 能力边界,见既有文档);
- 验证码期间 geetest 轮询吃满脚本预算会触发「页面脚本管线中断」(预算语义既有文档)。

## 六、验证

- 单元:`cargo test --lib` 2618 全过(新增回归 ×6:定时器→微任务同请求推进、管线内 XHR deliver、
  保险丝清除、显式同步、abort 抑制、管线定时器→Promise 链);
- 集成:`cargo test --test web_use_engine` 22 过 + 新增「动态脚本调用跨脚本注册的全局回调」;
  「富视觉页面」1 例为存量基线失败(干净 HEAD 同样失败,另线处理);
- 真站终态:captcha-sdk 注册→装载→fire cbs=1 → gettype 200(极验) → gt.0.5.0.js →
  fullpage.9.2.0 → JSONP 回调执行 → **滑块组件注入 #rowCode**;布局几何与 Chrome 一致。
