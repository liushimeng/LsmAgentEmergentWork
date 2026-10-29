# 37 · GeeTest 自愈循环可观测性与 HITL 可见性守卫

> 日期:2026-10-29
> 方案:`tmpPlan/20261029_123527_zhipin安全验证页GeeTest自愈循环与HITL可见性.md`
> 场景:`Dir_LsmAgentBrowser_zhipin_01` 真机任务(BOSS 直聘采集,`laew --debug -f`)。入口被 302 到安全验证页后,GeeTest 组件呈**非确定性自愈重置循环**,页面只显示「配置错误请点击重试116」;同时 `-f` 后台任务对隐藏模式 Chrome 页发起 `request_human`,弹窗却指示人工「在浏览器窗口完成验证」——人工无窗口可操作,300s 白等。

## 一、本轮实测确认的 GeeTest 链形态(Lsm 内核)

34/36 轮修通构建链之后,同一验证页在内核仍有三种形态(同一会话不同轮次可交替出现):

| 形态 | 表现 | 网络特征 |
| ---- | ---- | ---- |
| A 干净 | `geetest_holder geetest_wind geetest_ready`,雷达按钮 440×44「点击按钮进行验证」 | gettype/get.php 各一次 |
| B 循环 | 「配置错误请点击重试116」;console `Error UserCallBackError: 用户回调函数执行异常`(GeeTech monitor beacon:error_code=604, exception_url=./usercallback/init) | `api.geetest.com/reset.php` 以**每次新 challenge** 连打 19-20 次(间隔 5-6s),常把 120s 脚本管线预算烧穿 |
| C 循环后自稳 | reset.php ×20 后停在 ready | 同 B,但未超预算 |

循环驱动在 GeeTech fullpage 内部(自动点击雷达 → bind 弹层 → get.php(w= 指纹)被服务端拒 → reset);zhipin captcha-sdk@5.1.3 的用户回调同步段 `n.onReady(...).onSuccess(...).onError(...).onClose(...)` 链**无 try/catch**,任一环 TypeError 即被 GeeTech 吞成 UserCallBackError。Chrome 同页 `geetest_ready` 正常——对照确认这是内核指纹保真度与 canvas 能力边界问题,不是站点硬封 IP。

## 二、落地改动(通用,无站点特判)

1. **`LAEW_JS_CB_TRACE=1` 回调异常透传陷阱**(`script/prelude.rs`、`script/runtime.rs`):window 代理 get 侧对入口全局函数(`initGeetest` 等,`__lsmCbTraceNames` 可扩展)懒包装——函数参数与返回对象方法换成「`[cbtrap] 作用域 异常: message | stack` 进 console 后**原样重抛**」。SDK 内部 catch 语义不变,但原始异常进 `inspect(console)` 台账。默认关闭零开销。对照既有 `LAEW_JS_CHAIN_TRACE`(装载链打点)构成两级诊断。
2. **page_health `request_loop` 信号**(`browser/inspect.rs`):网络台账按「method + host+path(去 scheme/query)」聚合,≥8 次报出 `{method,path,count}` + `request_loop_note`(「疑似验证码组件反复重置/轮询循环…刷新通常无济于事」)。真站实测:`{"count":20,"path":"api.geetest.com/reset.php"}`。仅诊断不改行为;正常长轮询站点也会命中,故文案用「疑似」并引导结合 console/network 判断。
3. **request_human 人工可见性守卫**(双工具,`tools/mcp_web_lsm_browser_use/control.rs`、`tools/mcp_chrome_use/mod.rs`、`browser/mod.rs::viewer_alive`):
   - Lsm:查看器在位(headed 且存活)才算可见;Chrome:实例模式 headed 才算可见;
   - 不可见时:展示消息自动追加「[系统提示] 当前浏览器为隐藏模式(无可见窗口)…」,结果三态(应答/超时/取消)均回 `page_visible:false`;Lsm 超时分支 hint 改为「先以有头模式打开页面再重试 request_human」;Chrome 侧加 `visibility_hint`。
   - 目的:把「不可能指令」变成「模型可执行信息」——改 `open(mode=headed)` 重开页面,而不是反复发人工介入。

## 三、离线回归(tests/web_use_engine.rs)

- `cb陷阱_用户回调异常透传console`:mock 站模拟「SDK 入口函数 + 内部 catch 吞异常」,断言 `[cbtrap] initGeetest/cb` + 原始 message 进 console。
- `page_health_同路径请求循环探测`:9 次同路径 fetch(query 变化)断言聚出 `count≥8`、path 含端点。
- 套件 25/25。

## 四、边界(后续阶段,勿当已支持)

- Canvas 远程图片入画 / 路径 / 文字 / 渐变:GeeTest 点选题面画不出,该题型人工也无法完成(走 Chrome);WebGL 同理。
- `w=` 指纹保真度:形态 A/B/C 的非确定性来源之一,需指纹层专项。
- zhipin 服务端 IP 信誉墙(入口 302 verify.html)对 Chrome 同样生效,属站点风控;内核侧只负责「看得清」,不负责绕过。
