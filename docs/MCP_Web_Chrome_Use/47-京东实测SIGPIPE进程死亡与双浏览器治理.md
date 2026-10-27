# 47-京东实测SIGPIPE进程死亡与双浏览器治理(2026-10-27)

> 实测方式:`LAEW_CHROME_MODE=headed laew --debug -f prompt.md`,工作目录
> `/Users/dev02/Desktop/MyLocalGit/LsmBashDir/MCP_Web_Chrome_Use_01`。
> 任务:京东登录(HITL 扫码)→ 1 万元台式机选型采集 → 评论核查 → 产出
> `1W-组装机方案.md`。
> 结果:**运行 47 分钟(09:07–09:54)后被 SIGPIPE 整体杀死(exit 141)**,
> Yolo→Plan→Main-Work(8 单元/6 层)推进到 layer 3 末(wf-1~4 完成、wf-5 QC 中)
> 全损终止;HITL 扫码登录闭环正常,Chrome 由 watchdog 正常回收无孤儿。
> 方案文档:`tmpPlan/20261027_100434_京东实测SIGPIPE进程死亡与双浏览器工具治理方案.md`。
> 前篇:`45-实测缺陷闭环与双浏览器能力对齐.md`(D1~D4)。

## 一、本轮事故与缺陷总览

| # | 缺陷 | 严重度 | 状态 |
|---|------|--------|------|
| E1 | SIGPIPE=SIG_DFL 无条件恢复,长跑进程被子进程管道对端死亡直接杀死 | **P0(崩溃根因)** | 已修(F1) |
| E2 | `request_human` 的 reason 枚举未进 schema,模型首调必 1001 | P1 | 已修(F2) |
| E3 | 任务指定的浏览器工具未传导到执行单元,SubAgent 系统性双开内核 | P1 | 已修(F3) |
| E4 | `eval_js` 顶层 return 非法:每个新 SubAgent 重新踩坑 + 异常埋在 Debug 转储 | P1 | 已修(F4) |
| E5 | 并行单元互 close 对方页面(close(all) 可全灭兄弟单元) | P1 | 已修(F5) |

**G1~G7(44 篇)与 D1~D4(45 篇)的修复在本轮全部保持有效。**

## 二、E1 崩溃根因链(证据齐全,四环)

1. `src/main.rs` 启动即 `crash::restore_sigpipe_default()`——把 Rust 默认的
   SIG_IGN 改回 SIG_DFL,**所有模式无条件生效**(动机是 `laew provider list | head`
   的 Unix 管道惯例)。
2. layer 3 并行单元(09:32:30)给 `MCP_Web_LsmAgentBrowser_Use open` 传了
   `mode:"headed"`——引擎切有头,**拉起查看器子进程 `laew __browser-view`**
   (open 回包 `mode:"headed"` + `mode_hint:"有头模式已启用:独立查看器子进程…"`)。
3. ~09:54 查看器窗口死亡(人工关闭或崩溃);查看器与引擎之间是 stdin
   长度前缀帧管道,`page_bars` 1Hz 心跳 + 渲染帧持续写。
4. 引擎写帧 → EPIPE → **SIG_DFL 直接杀进程**(exit 141,无 panic 无 CrashDump),
   `view/mod.rs` 里 `write_all().is_ok()` 的「查看器退出自动回退纯无头」容错
   根本来不及执行。

**为什么查看器会被拉起**:任务只要求 Chrome;SubAgent 双开 Lsm 内核(E3),
且 Lsm 工具 schema 的 `mode` 参数 `default:"headed"` 诱导模型显式传 headed。

## 三、修复内容(全部通用,不绑定京东)

### F1 SIGPIPE 分流(crash.rs / main.rs / bash.rs)
- 长跑模式(TUI/`-p`/`-f`)在 CLI 分发处显式 `ignore_sigpipe()`:子进程管道
  EPIPE 成为普通 `Err` 交还各写点既有容错(view 回退纯无头、mcp transport 报错)。
- 短命 CLI 路径(provider/mcp/sessions/version/help + 三个隐藏子命令)保持
  启动期 SIG_DFL,`| head` 惯例不变。
- panic hook 识别「Broken pipe」类 panic(`println!` 在 SIG_IGN 语义下对已关闭
  管道的 panic)→ 静默退 141,不生成 CrashDump。
- Bash 工具子进程 `pre_exec` 恢复 SIG_DFL:宿主 SIG_IGN 会被 exec 继承,
  不恢复的话 shell 里 `grep … | head` 在 head 退出后会对端报
  "write error: Broken pipe" 而非静默退出。

### F2 request_human 文档化 + reason 宽容归一(human_assist.rs / 两工具 schema)
- Chrome/Lsm 两工具 `params` 补齐 `reason`(enum)/`message`/`options`/
  `multi_select`/`timeout_ms` 定义;Chrome description 补 request_human 用法句。
- `request_human_action` 前置 `normalize_reason`:自由文本按中英关键词归一
  (扫码/qr→qr_login;短信/sms→sms;验证码/captcha→captcha;登录/login→login;
  人脸/双因素/2fa→two_factor;授权/oauth→oauth;人工/核验→manual_verify),
  归不中才 1001+allowed 兜底。实测原文「京东要求登录后才能访问搜索结果/价格/评论页」
  直接归一为 `login`,首调即成功。
- Lsm schema 的 `mode` 参数描述改为「默认按交互性自动判定,不传即可;
  自动化任务勿传 headed(会在本机拉起查看器窗口)」。

### F3 任务级浏览器工具指定传导(orchestrator/workflows.rs)
- 新增 `browser_mandate(prompt)`:正向存在性检测(Chrome/chromium/谷歌浏览器→
  "chrome";lsmagentbrowser/自研内核/反反爬→"lsm";双指定/无指定→None)。
  旧 `mandated_tool` 只认「只使用/必须使用」句式,本轮任务原文
  「分别使用 Chrome 浏览器」判不中——回归对照测试固化。
- `build_subflow_input` 对**所有单元**注入硬约束段:「本单元全部网页操作必须
  使用 MCP_Web_Chrome_Use,禁止改用另一内核(会话/登录态不共享,放大风控),
  唯一例外=反复被风控拦截且无法推进」。

### F4 eval_js 自动 IIFE 包裹 + 异常人性化(agent/js_preprocess.rs + 两内核)
- 新模块 `js_preprocess`:检测**顶层** return(行首语句 + 花括号深度 0 近似;
  误差方向只有漏判,漏判时不包裹行为与旧版一致)→ 自动包 `(function(){…})()`。
  顶层 return 的脚本本就必然编译失败,包裹语义等价。Chrome/CDP 与 Lsm/Boa
  两条 eval 路径共用。
- Chrome 侧 JS 异常从 Rust Debug 转储(700+ 字符,`SyntaxError: Illegal return
  statement` 埋中段)改为 `Uncaught {class}: {desc} ({line}:{col})` + SyntaxError
  时附 IIFE 提示。

### F5 Chrome 页面所有权 + close 保护 + 孤儿页收养(chrome_browser/mod.rs)
- 注册表值 `Arc<Page>` → `PageEntry{page, owner}`;`PageOwner{agent, started_ms,
  weak}` 的 `weak` 是打开者 `web_activity` 台账弱引用——`upgrade` 成功=拥有者
  单元仍在运行,Drop 后=孤儿页。
- `close(指定页)`:目标页 owner 活跃且 ≠ 当前调用者 → 1001 拒绝(附 owner_agent
  与提示);`close(all)`:存在其它活跃 owner 的存活页 → 1001 拒绝(防并行层
  连 Chrome 进程一起回收)。仅调用者自身在 Agent 上下文时生效(测试/非编排
  路径不受限)。
- `list` 回包增 `owner:{agent,alive}` 视图;`open` 同 URL 且无主/owner 已死的
  孤儿页**收养复用**(所有权转移),owner 活跃则开新页(避免并行互踩导航)。
  治理页面累积(实测页面 id 涨到 c_00000014+)。

## 四、验证

- 单测:js_preprocess 5 例(顶层/函数内/回调行首/单行 if/变体)、normalize_reason
  8 断言(京东实测原文必须归一 login)、browser_mandate 3 例(实测原文必须命中
  chrome + mandated_tool 回归对照)、page_owner 判活与指针判等(Weak 模拟单元生死)。
- `cargo build` + `cargo test`:单元 2508 全绿;集成 protocol_wire/agent_loop/
  cli_provider/project_context/web_use_engine 全绿;`macos_ax_smoke` 1 例为
  本机既有环境存量失败(TCC 授权,与本轮无关)。
- 真机冒烟:`provider list | head`(SIG_DFL 短路径)exit 0;`laew -p`(SIG_IGN
  长路径)端到端正常。

## 五、实测经验(补充进站点行为知识)

- 京东搜索页 DOM 已改版:`li.gl-item` 计数 0,商品卡根节点 `[data-sku]`,
  标题/价格类名带 CSS modules 哈希前缀(`[class*="goods_title"]`/`[class*="price"]`),
  广告卡靠卡片 innerText 含「广告」识别。
- 京东详情页 `.p-price` 首屏可能命中「最近感兴趣」推荐位价格(¥89.00)而非
  本体价——需延迟等待 + 多选择器兜底,或以搜索卡价格交叉核对。
- 评论 JSON 接口 `club.jd.com/comment/productPageComments.action?productId={sku}
  &score=0&sortType=5` 直接返回纯 JSON(23K+ 字符),是最稳的评论采集通道。
- Lsm 内核对京东搜索/详情全部重定向到 `cfe.m.jd.com/privatedomain/risk_handler`
  「京东验证」页——京东属 Chrome 优先站点(与 41 篇结论一致:Lsm 价值在
  「不被风控打标」,但京东把无登录态/无指纹的内核直接打进了验证墙)。
- 顶栏 `#ttbar-login-2024` 内出现 `a.nickname` 即已登录,出现 `a.link-login`
  「你好，请登录」即未登录——可作为登录态机械判据。
