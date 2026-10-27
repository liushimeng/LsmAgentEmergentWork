# 28 - zhipin 站点全链路兼容与动态脚本同步装载

**日期**:2026-10-27
**场景**:BOSS直聘(zhipin.com)首页与搜索页(`/web/geek/job`)在 LsmAgentBrowser 内核中
永久停在「加载中,请稍候」、首页视觉渲染大面积空白错乱。
**结论**:网络层与请求头合成层已过 WAF(裸 curl 拿到的是「安全验证」页,内核拿到真实页面),
全部根因在 JS 执行兼容链与持久上下文存活策略。修复后搜索页完整渲染出「登录后查看更多职位」
界面,首页导航/职位卡片/企业区栅格全部正常。

## 一、失败链全景(按依赖顺序)

站点技术栈:jQuery 1.12.2 + Vue 1.0(vue-core)+ bossui(iView 系)+ rspack/webpack 分包
+ patas(APM 采样)+ ka/warlock(安全指纹)+ geetest(验证码)。

```
jQuery ──┬─ "onsubmit" in window = false ──→ IE 兜底分支 ──→ div.attributes[c].expando 崩
         └─ implementation.createHTMLDocument 缺失 ──→ 崩
Vue 1.0 ── document.createEvent("Event") 缺失 ──→ 崩(性能校准)
bossui ─── axios <a> 元素 URL 判同 ──→ a.pathname.charAt(0) undefined ──→ 崩
app~3 ──── URLSearchParams → __lsmDecode 未定义(本仓库笔误)──→ 崩
defer 序 ── app 入口(defer)先于 vue-core(阻塞)执行 ──→ Vue is not defined
挂载 ───── document.createComment('v-if') 缺失 ──→ 崩
chunk ──── 动态注入 <script src> 跨轮装载,虚拟时钟快进超 webpack 120s 超时 ──→ ChunkLoadError
SDK ────── document.elementFromPoint 缺失 ──→ patas/ka 定时器反复抛
存活 ───── eval/dispatch 超时即 clear_js ──→ 持久上下文被核爆 ──→ 页面状态归零
```

## 二、修复要点

### 2.1 DOM/Web API 补齐(PRELUDE)

| API | 语义 | 关键调用方 |
| --- | --- | --- |
| `element.attributes`(NamedNodeMap) | 数字/名称下标 + length + getNamedItem/item/setNamedItem/removeNamedItem,Proxy 活视图(按需读当前 attrs 容器) | jQuery support/removeAttr |
| `document.implementation.createHTMLDocument` | 轻量沙箱文档;body 为动态元素,childNodes 活视图使 `两 form → length 2` 探测为真 | jQuery parseHTML |
| `document.createEvent` | initEvent/preventDefault/stopPropagation | Vue 1.0 性能校准 |
| `document.createComment` | nodeType 8 占位注释 | Vue 1.x VDOM v-if |
| `document.elementFromPoint` | 快照节点最小包含盒命中,兜底 documentElement | patas 空白采样 / ka 指纹 |
| `<a>` URL 反射 | href 赋值即解析(`__lsmParseUrl`,注意先剥离 `//` 授权段前缀),protocol/host/hostname/port/pathname/search/hash/origin 可读 | axios 判同模块 |
| `window` Proxy `has` 陷阱 | `k in target \|\| k in globalThis \|\| /^on[a-z][a-z0-9]*$/.test(k)` | jQuery `"onsubmit" in window` |
| 动态元素 childNodes/children 活视图 | `(htmlKids).concat(kids)` | createHTMLDocument 探测 |
| `__lsmDecode` | URLSearchParams 安全百分号解码(裸 decodeURIComponent 遇孤立 % 抛 URIError) | new URL().searchParams |

### 2.2 defer 语义(管线执行顺序)

真实浏览器:解析期按序执行阻塞脚本 → 解析完 → defer 队列(文档序)→ DOMContentLoaded。
此前管线无视 `defer` 全按 DOM 序跑,zhipin 的 vue-core/bossui(阻塞)写在 defer 块**之后**,
defer 的 app 入口反而先执行。

- `collect_scripts` 产出 `items`(阻塞)+ `deferred`(`defer` 属性或 `type=module` 的外链);
- `run_pipeline` 每轮 `items` 在前、`deferred` 整组后置,组内保持文档序;
- 内联脚本与阻塞外链的相对顺序不变,与浏览器一致。

### 2.3 动态脚本线程内同步装载(核心)

**问题**:webpack/rspack 注入 `<script src>` 后挂超时定时器(120s)等 onload。宿主轮次机制
(序列化 → collect → fetch → 执行 → fire)要跨轮往返,虚拟时钟在等待期被快进,
装载器超时先到 → `ChunkLoadError` → 异步路由整条放弃。

**方案**:`__lsmRegisterScriptLoad`(appendChild/insertBefore 注册点)对 `<script src>`
**当场**经 `__lsmFetchNative` 同步取回 + `(0,eval)` 执行 + `__lsmFireScriptLoad`,
load 信号在回调返回前就发出。要点:

- 只对 `script` 元素触发(img 等替换元素同样带 src,误装载会把图片二进制塞进 eval);
- 「先 appendChild 再设 src」模式由 `src` setter 触发;
- `blocking_fetch` 新增 `script:true` 选项:请求头按 `Dest::Script` 合成(无 Origin),
  响应免 CORS 校验(经典脚本不受 CORS 约束;JS 响应体照旧过 for-in 守卫);
- 取回/执行经原生层自动进网络台账;执行完打 `data-lsm-executed` 标记,
  宿主 `collect_scripts` 见标记跳过,不二次执行;
- 非 http(s) 目标(about:/data: 占位)直接当空载成功。

### 2.4 持久上下文存活(铁律级修复)

**问题**:`js_eval_thread`/`js_dispatch_thread` 请求超时(默认预算 10s)即 `clear_js`,
管线轮次超时同样丢弃 JsHandle。Boa 下重站单轮执行数十秒是常态——线程是**忙**,不是死;
清句柄等于把页面全局状态(webpack 模块表/Vue 实例/jQuery)整页核爆,后续 eval 在新
Context 上白手起家,页面永久停在残缺态。

**方案**:三处统一改为「只有 `JS 线程已退出`(sender 已关)才清句柄;忙碌超时保留」:
`js_eval_thread`、`js_dispatch_thread`、`run_pipeline` 轮次超时。线程跑完本轮后,
后续请求(交互补齐 run_pending_scripts / eval)继续复用同一上下文。

### 2.5 诊断可见性

`__lsmRunScript` 捕获异常原先只回传 `e.stack` —— Boa 栈帧不含 `TypeError: xxx` 消息行,
压缩大脚本只剩帧无从定位。现保留 `String(e)` + 栈(消息行在前,与 DevTools 习惯一致)。
排障技巧:`LAEW_JS_FORIN_GUARD=off` 跑探针可让报错列号与原始文件 1:1 对齐。

## 三、验证与回归

- 2565 单测全绿(新增 10 个 zhipin 回归:in 陷阱/两路 attributes/createHTMLDocument/
  createEvent+createComment/defer 后置/URLSearchParams/a 反射/elementFromPoint/img 不触发装载);
- web_use_engine 等集成测试全过;
- 真机:搜索页渲染「登录后查看更多职位」完整界面;首页错误 34→1,布局正常;
- 截图(视口/全页)无缺陷。

## 四、边界与后续

- 搜索页职位列表需登录:Agent 流程 `inspect(blockers)` → `request_human`(HITL)→ 登录后继续。
  引擎侧已从「死在加载中」变为「正常渲染登录墙」。
- `calc()` 不支持(本站 52 处)与 `position:sticky` 近似是当前样式引擎已知边界,见 tmpPlan 方案。
- `data:font/woff` 走图片解码通道会报 WARN,属噪声,后续可按 MIME 静默。

## 五、渲染引擎优化追加(calc / 视口单位 / box-sizing)

复核时 zhipin 已被服务端 IP 打标(首页退化为「安全验证」页,真 Chrome 同款),
借此轮落地渲染引擎三个真实缺口:

1. **`calc()` 全量支持**:`parse_length` 识别 + `eval_calc` 递归下降求值
   (加减乘除/括号/一元负号/内嵌 calc 展开/全部单位;`%` 沿用视口宽近似,
   与 `width_percent/height_percent` 精确通道互不干扰;除零与非法表达式
   回退 None)。zhipin 全站 48 条 calc 规则(min-height/max-height/top/width/margin)此前整条被丢。
2. **视口单位 `vw/vh/vmin/vmax`**:`height:calc(100vh - 400px)` 类全屏容器此前全部塌缩。
3. **`box-sizing`**:布局默认按 border-box(全局重置的既成事实语义);显式
   `content-box` 时宽高按内容盒还原(padding 加回),块级/flex/grid 三路同步;
   `getComputedStyle` 反射计算值。

验证:本地端到端渲染(calc 100%-40px=1240 / 100vh-400px=400 / 50%-20px=600 /
50vw=640 / 20vh=160 全部按真值渲染,截图留档 `probe/calc_render.png`);
2570 单测 + 21 引擎集成全绿;WAF 页无回归。

## 六、站点侧观察(IP 打标)

zhipin WAF 为 IP 信誉打标:同日几十次全量加载(测试/探针累计)后,首页/搜索页
均退化为「安全验证」页(captchaPage_ip 形态,与猎聘同款)。内核无法绕过服务端
打标;治理思路(熔断记账/冷却/换指纹)属站点治理层,见 `docs/双浏览器工具架构/`
与历史轮次方案。当日 IP 已被打标时,任务侧应及早 `request_human` 让人工过验证,
或改日重测。
