# 53 - 猎聘加载遮罩白屏 HITL 软化与首页 hero 水合断链

> 2026-11-04 真机任务实录(MCP_Web_LsmAgentBrowser_Use_01,猎聘 AI 岗位采集)。
> 两个独立问题:① zhaopin 搜索页整幅白屏导致人工介入死锁;② 首页 hero 整幅
> 背景色空块(布局显示不全)。

## 问题一:站点加载遮罩 × 人工介入死锁(已修复)

### 现象与取证

- 查看器窗口在 `liepin.com/zhaopin/?dqs=010&key=AI` 整页白屏;`request_human`
  弹窗后人工对着白屏两次点「我已完成」,页面登录态始终未变,任务死锁。
- `LAEW_DUMP_DISPLAY` 显示列表铁证:页头/筛选栏/登录表单(1168,248,384×278)/
  「出错了(400)」toast **全部正常布局**(#0-#122),但 `#123 bg(白) 1920×1080
  z=9999` 整幅不透明遮罩盖在最顶层 + 三个 14×14 半透明橙色圆点(loading 动画)。
- 来源是站点自己的 CSS:
  `.searchjobs-spin-loading-box{background:#fff;height:100vh;position:absolute;z-index:9999}`
  (antd Spin 包一层站点自定义白底)。
- `eval_js` 祖先链:遮罩父链无任何定位祖先(`.common-page-container` static)→
  真实 Chrome 里 `position:absolute` 同样锚定初始包含块(全视口)——**内核布局
  正确,Chrome 里这遮罩也是全屏的**。
- console:`Unhandled promise rejection Error: 出错了(400)` ×N —— 搜索 API
  400(未登录/风控),站点 React 错误路径跑了(toast 都渲染了)但 loading 态
  永不翻 false,遮罩永驻。**站点状态,非内核渲染故障。**

### 死锁链与修复

死锁链:API 400 → 遮罩永驻 → `request_human` 交还人工控制权 → **人工看到的
仍是白屏,遮罩下的登录表单不可见** → 人工只能空点「我已完成」→ Agent 复查
未登录 → 再次 request_human → 循环。

关键事实:`human_hit_test` 按**面积升序**取候选,登录输入框(小盒)天然优先于
全屏遮罩(大盒),**人工点击命中本就不受遮罩影响——唯一阻断是视觉**。

修复(通用,非猎聘特判):

- `page_bars::soften_loading_overlays`:复用 `inspect::is_loading_overlay_item`
  判据(不透明背景 + ≥95% 视口 + 严格高于全部内容 z),把命中项 opacity 降
  到 0.25;`detect_loading_overlay` 拆出 `loading_overlay_content_max_z` /
  `is_loading_overlay_item` 两个 `pub(in browser)` 助手,单一真源。
- `view/bridge.rs::push_frame`:控制权为 `User` 且检出遮罩时,克隆 `Layout`
  软化后渲染查看器帧(仅此罕见路径付费),叠加琥珀胶囊
  「站点加载遮罩 · 已临时半透明以便人工操作」。**Agent 控制态/取证截图
  (`screenshot_locked` 不经 page_bars 层)保持站点原貌,证据链不受污染。**
- 真机验证:2026-11-04 11:15 request_human → 用户态 → 查看器胶囊在位,
  页头/登录表单/hero 透过 25% 白纱清晰可见。

### 排障方法论沉淀

1. **截图问题先像素统计再定论**(magick mean/std/逐点采样):本轮两次把
   「正确渲染」脑补成「登录组件乱排」——AI 读图会把稀疏字形脑补成完整布局,
   4x 放大 + 单字验证也未必免疫;`magick compare -metric AE` 差分 + 单色
   包围盒扫描才是一锤定音。
2. 「DOM 有内容/渲染全白」先 `LAEW_DUMP_DISPLAY`:显示列表 z 序直接暴露
   遮罩项,比截图反推快一个数量级。

## 问题二:首页 hero 整幅背景色空块(布局显示不全,根因=水合断链)

### 现象与取证

- 引擎截图像素验证:hero 区(0-600px)为**纯藏青 (0,6,66) 零内容**;Chrome
  参照(同 URL 未登录)hero 有完整标题/特性卡/登录卡。
- dump html:hero 是**空脚手架**——`<div id="home-banner-bg-root"></div>` +
  空 ad slot + `<div class="home-banner-login-box" id="home-banner-login-container"></div>`
  (容器全空),内容全靠 JS 水合;静态部分(AI时代新机会/职位卡/热门城市)正常。
- 站点按指纹分流:给我们的引擎下 **SEO 简化版**(无登录横幅 SSR),Chrome
  headless 拿到交互版。SEO 版 hero 水合失败 → 空藏青块。
- **600s 大预算探针(v6)仍空**:`home-banner-login-box` 高度恒 1.0,
  console 4 条错误——水合链是**断**(JS 异常),不是慢。
- release 真机任务二轮:`脚本管线结束 rounds=1 executed=18 budget_cut=true`
  ×4(首页一轮 97s 成功、zhaopin SEO 页被掐)。

### 已落地修复

1. **基线管线预算 240s → 360s**(`PIPELINE_BUDGET_MS`):管线轮等待=剩余墙钟,
   基线放大直接扩大水合轮等待窗口(京东 222s、猎聘 240s+ 都是同一形态的
   「重站脚本在 Boa 下就是慢」);快站不受影响(跑完即收工)。
2. **page_health `script_cut` 诚实化**:原判据 `budget_cut && pending>0` 会漏报
   「脚本全取完但定时器水合被墙钟掐断」的形态(猎聘实测 pending=0 照样截断),
   改为 `budget_cut` 单条件,提示语区分 pending>0(补泵续跑)与 pending=0
   (定时器被掐,重开/调大预算/换 Chrome)。
3. 顺延条件**不做**放宽:「上一轮有产出动作就顺延」分不清「卡死脚本的部分
   动作」(京东 pc-tk)与「水合推进」(猎聘),会破坏京东超时截断+补泵语义
   (tests_jd_compat 有契约用例)。

### 遗留边界(下轮跟进)

- 首页 hero 水合的**具体 JS 断链异常**待 v7 探针抓取 console 错误文本定位
  (历史同类:174/177 轮猎聘 jQuery/React 链、zhipin 兼容链——通常是少数
  标准 API 缺失,补 PRELUDE 可解)。
- 站点给引擎下 SEO 版的分流维度(UA/指纹)待确认:引擎 UA 是 Chrome/128,
  与真 Chrome(更高版本号)不同,站点可能按 UA 新旧分流;升 UA 版本号可能
  直接拿回交互版,值得一试。

## 相关文档

- `docs/双浏览器工具架构/02-屏蔽页面状态栏与进度栏.md`(查看器帧合成层先例)
- `docs/双浏览器工具架构/03-浏览器控制权状态机与视口自适应.md`(控制权两态)
- `52-猎聘白屏根因与样式引擎性能优化.md`(上轮猎聘:站点遮罩诊断 + 样式引擎 4.5×)
