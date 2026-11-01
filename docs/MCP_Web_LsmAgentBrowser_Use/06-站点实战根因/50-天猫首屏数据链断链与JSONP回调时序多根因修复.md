# 天猫首屏动态模块空:JSONP 回调时序与 open 契约多根因修复

## 背景

2026-11-01 天猫组装机真机任务（`LsmBashDir/MCP_Web_LsmAgentBrowser_Use_tmall_01`，
`llaew_20261101_082148.log`）：

- 首屏渲染对比 10-30 轮有进步:SSR + 顶部导航(含「亲,请登录免费注册」登录入口)+
  搜索框 `#q`(792×34 @688,54 已布局)+ 页脚均正常;但**分类面板、商品楼层、页头
  用户区仍为空**;
- `inspect(console)` 12 条 error,主链:`ReferenceError mtopjsonptbpc1 is not defined`
  (mtop JSONP)+「页面脚本管线中断:JS 执行超时」×6;
- `open(timeout_ms=45000, wait_until=domcontentloaded)` 实际 **282.9s** 才返回。

本文记运行时/工具契约根因;登录 UI 不可达属能力边界(见「已知边界」)。

## 根因链

### 1. mtop JSONP 回调断链(eager 执行时序)

动态 `<script src>` 为 zhipin ChunkLoadError 语义改成**注册即同步取回执行**
(`prelude.rs::__lsmLoadDynScript`),而 mtop JSONP 的调用方在 `appendChild`/设 src
**之后**才注册回调 —— 浏览器里脚本异步执行天然错开该时序,eager 内核必
`ReferenceError mtopjsonptbpc1 is not defined`,`aldlamp`(楼层/导航数据)响应无处
落地,首屏动态模块整片空。

并列洞:window Proxy `set` 陷阱的全局同步条件是 `!(k in globalThis)` —— 同名回调
**再注册**时全局绑定滞留旧值;`delete window.foo` 也没有 `deleteProperty` 陷阱,
全局侧残留僵尸回调。

### 2. `script.text = code` 内联注入静默失效

站点注入内联脚本的经典写法 `s.text = code`:内核没有 `text` 访问器(应为
textContent 的 `[PutForwards]` 转发),代码写进普通属性后序列化丢失;且 `__serEl`
对正文一律 HTML 转义,`&&`/`<` 被转义坏 —— 即使序列化出来,宿主取回执行也必炸
(script/style 是 raw-text 元素,不得转义)。

### 3. `open` 契约与实现脱节

- `wait_until` **只在 JSON schema 里声明**,实现零消费;
- `timeout_ms` 只约束网络抓取,脚本管线独立吃 240s 预算 + 「有待取回脚本就顺延
  60s×2」续期 —— 调用方 45s 预算被拖到 282.9s(距工具级 300s 硬超时仅 17s,
  管线 360s 上限 > 工具 300s 的矛盾仍在,见「已知边界」);
- 截断事实只进 `page_health`,open 响应看不见。

### 4. 首屏动态模块空 = 1+2+管线截断叠加

管线反复「线程预算耗尽」(6 次,如实入账)+ JSONP 数据链断 + script.text 注入失效,
动态楼层/分类/用户区三路都到不了 DOM。

## 通用修复

落点:`src/agent/browser/script/prelude.rs`、`browser/mod.rs`、`browser/api.rs`、
`tools/mcp_web_lsm_browser_use/mod.rs`。

1. **JSONP 回调时序容错**:动态脚本体命中「绑定未定义」(ReferenceError /
   `is not defined`)时不判死,挂 `setTimeout(0)` 下一宏任务重试(≤3 次),成功照常
   发 load,耗尽才走错误路径;重试挂起期间缓存命中分支不提前派发结局。
2. **window→全局同步**:set 陷阱改为再注册覆盖(浏览器全局对象语义),新增
   `deleteProperty` 陷阱同步清绑定;`eval/Function/globalThis/location/document`
   五个内核命脉进保护名单不跟随覆盖(eval 被页面换掉会打断内核装载管线,
   location 是导航访问器)。
3. **`script.text` 注入**:`text` 访问器转发 textContent;`__serEl` 对 script/style
   正文按 raw-text 免 HTML 转义(浏览器 innerHTML 序列化同语义)。
4. **open 契约落地**(`NavHints`):`wait_until=domcontentloaded` → 管线 `max_rounds=1`
   首轮即返;**显式** `timeout_ms` → 整调用硬预算(抓取+管线共享,`hard_deadline`
   跳过 60s 顺延续期);不传维持旧行为(抓取 60s + 管线默认预算,重站点零回归);
   `script_budget_cut`/`script_pending` 随 open 响应直接上报。

## 验证

- `tests_jsonp.rs`(单测,JsHandle 真跑 prelude):JSONP 晚注册延迟重试送达、
  重试耗尽如实入账不悬挂、window 再注册覆盖/删除同步、`script.text` 正文原样
  进动作不被转义;
- `tests/web_open_budget.rs`(mock HTTP 端到端):显式 timeout_ms 夹紧 + 截断上报、
  `domcontentloaded` 首轮即返、不传 timeout_ms 旧行为不回归;
- 既有回归全绿:`web_use_engine`(26)、`web_script_drain`、`web_form_state_roundtrip`、
  `browser_canvas_screenshot`、`browser_mode_env`、`cargo test --lib`(2822)。

## 诊断方法

1. 首屏动态模块空先看 `inspect(console)` 的**首个** ReferenceError/TypeError,
   区分「回调断链」与「管线预算截断」(`page_health.script_budget_cut`);
2. mtop JSONP 判定:网络台账 200 ≠ 数据已交付,看回调是否 `is not defined`;
3. `open` 耗时异常时核对调用方 `timeout_ms` 与响应 `script_budget_cut`/`wait_until` 回显;
4. 内联注入不执行:查 `script.text` 访问器与序列化正文是否被转义。

## 已知边界

- **登录 UI 不可达(能力边界,非 bug)**:天猫登录框在 iframe `#J_loginIfr`
  (0×0 不渲染,capability_gaps 已上报);顶层 `login.tmall.com/member/login.jhtml`
  被重定向 `err.tmall.com/error1.html?c=404`(内核指纹被拒);Chrome 档
  `request_human(qr_login)` 300s 超时返 4001(hidden 模式 `page_visible=false`)。
  qr_login HITL 流程本身工作正常(二维码 PNG 落盘 + 提示语 + 超时语义)。
- 工具级 `LAEW_TOOL_TIMEOUT_SECS=300` 硬超时 < 管线缺省 360s 上限的矛盾仍在
  (显式预算路径已消解;缺省路径受 240s+120s 管线预算约束,重站点逼近 300s 时
  有整锅端风险)。
- `xhr rs: TypeError: cannot convert 'null' or 'undefined' to object` ×2 未修
  (站点代码与内核 XHR shim 形态差异,需本地化到具体调用点)。
- 观察入口 `explore`/inspect 在重站点不「快进快出」(实测 5 查询 98.7s),
  与补泵预算相关,留待后续。
