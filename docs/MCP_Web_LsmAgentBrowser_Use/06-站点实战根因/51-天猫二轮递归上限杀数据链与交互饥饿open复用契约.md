# 51 - 天猫二轮:递归上限杀数据链 / 交互饥饿 / open 复用契约

- 时间:2026-11-01 10:56–12:15
- 真机目录:`D:\MyLocalGit\LsmBashDir\MCP_Web_LsmAgentBrowser_Use_tmall_02`(诊断探针在 `probe/`)
- 方案文档:`tmpPlan/20261101_112500_天猫二轮交互饥饿与open复用契约修复方案.md`
- 上轮:50 号(JSONP 回调时序 / QC 整型 / thinking 污染 / open 契约)
- 终止原因:人工核验绕不过(HITL 300s 超时 + 查看器无页面键鼠通道)

## 一轮修复的真机回归结论

| 一轮修复 | 本轮真机表现 |
| --- | --- |
| open 契约(timeout_ms/wait_until) | `timeout_ms=60000` → 83.9s 返回(旧 282.9s 无视预算),`script_budget_cut/wait_until` 如实回显;探针 `timeout_ms=240000` → 261.9s |
| JSONP 回调重试(P0-1) | 间接验证:硬预算截断后 JS 线程**后台自建页面**,6 分钟后导航栏建成 |
| Plan/QC 链路(ee43ebe) | hard 任务实战通过:7 WorkFlow 全量解析、QC 十轮核查含日志交叉验证 |
| 工具超时自适应 | open 327.7s 未被误杀(浏览器工具自动放宽到管线+顺延+60s) |

## 本轮五组根因与修复

### R7 → P0-0:递归上限 256 杀死 mtop 数据机器(本轮最重要)

**证据链**(探针 console 13 条 + network 271 条):

1. `RuntimeLimitError: reached the maximum number of recursive calls` 栈 `tT→_i→_a→r→Request`(mtop.js 请求机器)×2;
2. 楼层数据 JSONP(`h5api.m.tmall.com/.../aldlampservicefixedresv2`)**网络层 4 次全部 200** —— 失败纯在客户端:回调由已死的 mtop 机器注册,JSONP 重试救不了「注册方已死」;
3. `MAX_RECURSION=256` 系 611c10f(arXiv 轮)为防失控递归自设,**低于 Boa 默认 512**,更低于真实浏览器(V8 承载数千帧);React/mtop 类站点常规递归深度即超 256。

**修复**:`MAX_RECURSION` 256→2048 + env `LAEW_JS_RECURSION_LIMIT` 可调(下限 512)+ JS 线程栈 32MB→64MB;回归测试「千层递归不被RuntimeLimit误杀」。

> 教训:给解释器加「防失控」硬限时,先想清楚重站点的**合法深度**——一个站点的护栏成了另一个站点的天花板。

### R1 → P0-1:交互饥饿族(管线截断后 eval/wait 全堵死)

- `eval_js` 连续 2002「线程忙碌」(每次烧 46.9s);`wait ms=2500` 实付 **117.9s**;
- 机理:`thread_main` 一次处理一个请求到完成,截断管线的**残留批次**在线程里跑数分钟,后续 Eval 在 mpsc 邮箱里挨饿;
- **修复:批内让道** —— `run_scripts` 脚本边界 + 排水事件循环的定时器批次间 `try_recv`,短请求(Eval/Diag)当场完整服务,长请求/Shutdown 暂扣到 deferred;现场保护三件套(REQ_ENV 快照恢复含 deadline 顺延 / PRELUDE `__lsmStashPending`/`__lsmRestorePending` 回填批次台账 / timer 字段由批次收尾重设)。语义 = 真实浏览器「devtools 求值插在页面任务之间,永不饿死」;单脚本内部不可抢占,eval 最坏等待从「整批数分钟」压到「单脚本时长」。
- wait 侧配套:**泵预算与调用方上限挂钩**(`sync_page_with_budget`/入口补泵 `interaction.min(total)`),2.5s 的 wait 不再先吃 30s 交互预算 + 循环 60s 全档 sync。

### R2 → P0-3:open 同 URL 复用 = 全量重导航(327.7s 黑盒 + 丢后台进度)

- Plan `open(url)` 命中复用 → `navigate_locked` 全量重载(240s 管线 + 60s 顺延 + 20s 图片 ≈ 327.7s),且导航 `shutdown` 旧 JS 线程,**首开后台自建的进度整页丢弃**;
- **修复:精确同 URL 默认廉价复用**(≤8s 补泵 + 返回现态,`timings.reused=true`),`params.reload=true`(别名 force)才全量重载 —— 与 Chrome 工具「同 URL 自动复用无主页」语义对齐;同源不同 URL 仍走全量导航。

### R3 → P1-4:整调用预算不含图片阶段(60s 预算 83.9s 返回)

- `load_images` 吃独立 20s 预算、样式预取按请求级 timeout,均不受 `NavHints.whole_call_budget_ms` 约束;
- **修复**:两条加载路径的样式/图片阶段都按剩余预算夹紧,归零时图片阶段整体跳过(skipped 如实记账);集成测试「显式timeout预算夹紧图片阶段」(慢主文档 2.6s + 慢图 6s,3s 预算收敛返回)。

### R4/R5 → P1-5:截断上报粒度与可观测性

- `budget_cut=true + pending=0` 无法反映「动态装载链未展开」(pending 只统计截断时 DOM 在场未执行脚本);
- `LAEW_DUMP_HTML` 只在建页路径落盘,navigate 复用路径**不落盘**;`script/mod.rs` 零 tracing,`--debug` 日志回答不了「管线跑到哪」;
- **修复**:open 响应带 `timings`(fetch/styles/pipeline/images/layout/total_ms + pipeline_rounds + scripts_executed,复用路径 pump_ms);管线逐轮 tracing + 收尾 tracing;dump 抽成 `dump_html_if_env` 两条路径共用。

### R6 → P1-6:HITL 能力不对称(查看器无页面键鼠通道)

- 查看器 `ViewCommand` 只有 `navigate/back/forward/reload/home/resize` —— **人工无法在窗口里点击/输入**;「请在浏览器窗口中扫码或账号密码登录」在 Lsm 引擎上是不可兑现指令(有头也一样);
- 探针佐证:顶层 `login.tmall.com/member/login.jhtml` 仍 302 到 `err.tmall.com/error1.html?c=404`(服务端风控,同 50 号轮),登录三路皆断(iframe 不渲染/顶层 err/无键鼠);
- **修复(话术如实化)**:request_human 消息按真实能力生成系统提示(观看+地址栏导航;凭据走弹窗转述由 `control(input_text)` 代填;扫码需 QR 可见且轮询在跑),响应/超时提示附 `input_forwarding:false`;查看器键鼠转发立项 P2。

## P2 遗留(本轮不修)

- aria.js 双执行(`data-lsm-executed` 未覆盖该注入路径);
- `@font-face` 的 woff/ttf 被 CSS url() 收集器当图片预取并「解码失败」告警(纯噪音);
- blockers 的 page_text 匹配是否含内联 script 文本(本轮「请登录」经查来自已建成导航,未见假阳性);
- 查看器页面级键鼠转发(IME/焦点/命中测试,需立项);
- 简单诊断任务走完整 Yolo→Plan→SubAgent 编排偏重(探针自发跑了三层)。

## 指纹速查

| 现象 | 指纹 | 根因 |
| --- | --- | --- |
| console `RuntimeLimitError ... recursive calls` + 栈入 `Request` | Boa 递归上限 | `MAX_RECURSION=256` 杀 mtop/React 链 |
| eval_js 连续 2002「线程忙碌」且每次 ~47-60s | 残留批次垄断线程 | 批内未让道(本号修复前) |
| `wait ms=小` 实付 ~118s | 30s 补泵 + 60s sync 全档 | 泵预算未与调用方上限挂钩(本号修复前) |
| 同 URL 二次 open 变 327s 黑盒、页面反而变空 | navigate 复用重启 JS 线程 | 复用未走廉价路径(本号修复前) |
| `timeout_ms` 显式传入仍超返回(60s→84s) | 图片 20s 独立预算在预算外 | 整调用预算未夹紧后置阶段(本号修复前) |
| dump_last.html 停在首开状态 | navigate 路径无 dump | dump 只在建页路径(本号修复前) |
