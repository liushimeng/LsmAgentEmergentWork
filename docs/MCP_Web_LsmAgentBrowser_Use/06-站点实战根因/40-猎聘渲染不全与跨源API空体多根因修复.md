# 40 - 猎聘「整体显示不全 + 搜索页职位全空」多根因修复

> 对应方案:`tmpPlan/20261030_103759_猎聘渲染不全与跨源API空体多根因修复.md`
> 场景:2026-10-30 真机任务(`Dir_LsmAgentBrowser_01`,`laew --debug -f` 猎聘 AI
> 岗位采集),用户反馈:**网站整体显示不全、只能看到部分导航栏文字、渲染功能
> 完全不达标**;并要求核验截图功能。
> 前置:38/39 轮已修页头空白/登录页白屏/管线丢回复/卡片几何;本轮在其后。

## 一、实测证据(修复前)

探针(`tests/tmp_liepin_probe*.rs` ×3,验完即删)+ 离线 dump 复算 + curl 对照:

| # | 现象 | 数据 |
| --- | --- | --- |
| E1 | 页头**双副本叠印** | 最终 DOM 同时含静态 SSR `ul.header-nav-menu` 与 React 重建副本(`data-lsm-id`),同容器并存;布局文本「首页 职位…」整段重复两遍,右侧「我是猎头/我是招聘方/登录/注册」互相叠印 |
| E2 | 页头**贴左不居中** | `.header-content-box{width:1192px;margin:0 auto}` 实测 x=0(应 x=364) |
| E3 | 搜索页正文**全白** | 显示列表 `#106 bg(白,不透明) x=8 y=68 w=1184 h=1080 z=9999` 最后绘制,盖住筛选栏/空职位提示/登录卡/页脚(布局层全在) |
| E4 | 坐标**天文数字** | 显示列表 4 项 `y=4.494e307`(=UNKNOWN_CB_H/2);身份:`.ant-spin-dot` 的 `top:50%` |
| E5 | 搜索页**职位列表空** | 全部 `api-c/api-wanda.liepin.com` POST:`status=200 bytes=0`;curl 同端点最小头即返回 55 字节 JSON(服务端正常) |
| E6 | 行业卡**标签药丸无字** | `.jobcard2-detail-labels` 内两个 inline-block span 同坐标 (40,940) 叠印;容器显式 `height:22px` 实测 37.4 |
| E7 | hero 背景空白 | `home-banner-bg-media-wrap` 内是 `<video src="…myqcloud….mp4">`;能力缺口未上报 video |
| E8 | `open` **误报 captcha 阻断** | 登录面板「获取验证码」按钮文案命中裸词 → `blocked=true` + `next_steps=request_human` 误导 |
| E9 | CSP 假拦截 | `script-src` 白名单里的 `www.googletagmanager.com` 被拦(控制台假告警) |
| E10 | 截图链路 | 视口/整页/clip/OCR/落盘全部正常——PNG 内容缺失是渲染层问题,非截图 bug |

关键对照(curl):API 响应带 `Access-Control-Allow-Origin: https://www.liepin.com`
但**不含** `Access-Control-Allow-Methods`(实际响应本就不带);台账里被
CORS/CSP 拒绝的请求恰好记 `bytes=0 status=200` —— 签名与 E5 完全吻合。

## 二、根因与修复(全部为通用内核缺陷,无站点特判)

### 1. CORS 对**实际响应**误查 `Access-Control-Allow-Methods` → 跨源 POST 全灭

`check_simple_request` 同时校验 origin 与 method,而 `allows_method` 在响应无该头时
恒 false。浏览器语义:**实际响应只查 ACAO;Allow-Methods 只在预检(OPTIONS)响应
里查**。于是所有跨源 API POST 被内核自己判死 → 页面 JS 走 error 通道 → 职位列表
空 →「非常抱歉!暂时没有合适的职位」。

修复:`check_simple_request` 去掉 method 校验(保留 origin);预检
`check_preflight_request` 完整三查不动。

### 2. 页头双副本 = React 生产版**水化失败静默回退**

DOM 全量账本(LAEW_DOM_TRACE=2):logo 水化成功(静态 img 收到 `set_attr src`),
city-box 处断链后 React 把重建的 city-box/ul/quick-menu 直接 `append_html` 进
`nav.header-content-box`,**全程无清空/移除动作**(477 条动作仅 1 条无关 remove_node)。
Chrome 水化成功复用静态节点;内核断链 → 静态骨架 + 动态副本并存叠印。

修复(三层):
- 快照元素 `replaceChildren()` 补 push `clear_children` 宿主动作(此前只清 JS 侧覆盖层);
- **id 唯一性守卫 → 结构级退役**:`append_html`/`insert_html` 应用前,片段携带的 id
  若宿主已存在,沿「宿主祖先链 × 片段祖先链」**锁步上溯**找结构对应点——同标签、
  同类名、且旧祖先子树里的 id 全部被新片段重新声明(完全被取代)的最高祖先对,
  把旧祖先**整棵退役**(重复 id 本就非法 HTML;水化回退后新子树才是框架持引用的
  活树;浏览器对水化失败同样「清旧挂新」)。**只退役 id 载体不够**(终验复盘:
  静态 `ul.header-nav-menu` 的 `a#header-nav-menu-*` 退役后,残余 li 壳仍带
  padding/margin 占位,把 React 重建的导航整体右推 340px,与右锚定
  (`position:absolute;right:0`)的 quick-menu 叠印成一团)。安全边界:旧子树存在
  未被片段重新声明的 id 时 Containment 阻断上溯,退回只退役载体;插入容器与
  body/html 永不退役。`ApplySummary.retired` 计数诊断。

### 3. 块级/flex/grid 容器 `margin:0 auto` 不居中

`parse_length("auto")=None` → auto 被丢成 0。`ComputedStyle` 增
`margin_left_auto/margin_right_auto` 标记(长/简写、1-4 值槽位识别);块级分支 +
`flex.rs`/`grid.rs` 的 `layout_container` 在显式宽度下按剩余空间解 auto
(双侧=居中,单侧=吃满)。离线复算:猎聘页头 nav x=0 → **x=364 精确居中**。

### 4. `UNKNOWN_CB_H` 哨兵泄漏为天文坐标

`top/bottom` 百分比对「包含块高未知」哨兵(f64::MAX/2)直接求解 → y=4.49e307,
污染整页高度与渲染预算。修复两层:`resolved_top/bottom` 对哨兵级包含块高
(≥1e15)返回 None(CSS 不定高按 auto);`compute_full` 收尾
`sanitize_layout_geometry` 把任何非有限/超界(>1e6)几何归零标不可见,页高钳
`PAGE_H_SANITY_MAX`(1e6)。

### 4b. z=9999 白遮罩 = 站点自身的 loading 层(非渲染 bug)

身份:`.ant-spin-nested-loading.searchjobs-spin-loading-box`
(`background:#fff;height:100vh;z-index:9999`)。Chrome 里数据到达即撤;内核里因
根因 1 数据永不到 → 白罩永不撤 → 搜索页正文全白。**修根因 1 后自动消失**,
不做任何站点特判。

### 5. 行内级兄弟不做水平推进 → inline-block 叠印

块容器 `layout_children` 的行内子级分支与 `collect_inline_children` 都给每个子元素
传**同一个起点**,inline-box 发射后不推游标 → 兄弟同坐标叠印(药丸标签两 span
同在 (40,940);文本与随后的盒同理)。修复:两处维护行内游标 `pen` ——
inline-box 子级按 margin-box 宽推进;文本/纯 inline 子级按字宽推进并钳到容器
右缘(`font-size:0` 容器的空白推进 0,与站点去缝手法语义一致)。

### 6. `<video>` 能力缺口未上报

`open` 的 `capability_gaps` 增 `video` 类:计数 + 「内核不渲染视频画面,hero 背景
视频区域留纯色/空白;DOM/文本不受影响」+ Chrome 工具回退 hint(与 webgl/iframe
同款)。

### 7. `blockers` 裸词「验证码」误报

captcha 词表收紧为带验证动作语义的完整短语(「请输入验证码」「请完成验证」「滑动
验证」「人机验证」「安全验证」「点击按钮进行验证」…),删除裸「验证码」——登录
面板「获取验证码」按钮文案不再触发误阻断。

### 8. CSP 裸主机源跨 scheme 不匹配(假拦截)

`is_allowed` 对 host 源只做 `url.starts_with(src)`;CSP 裸主机源(无 scheme)对
`https://` 前缀 URL 恒 false → 白名单脚本被假拦截。修复:解析 URL 主机与裸主机源
(含 scheme 剥离)精确比对。

## 三、验证

- 单测 2702 项全绿(新增:CORS 实际响应不查方法/预检仍查、replaceChildren 清宿主、
  id 结构级退役/Containment 边界、margin auto 居中/单侧、inline-block 排开、哨兵
  返回 None、CSP 裸主机);
- `web_use_engine` 26 项、`web_form_state_roundtrip` 2 项、`browser_canvas_screenshot`
  1 项、`browser_mode_env` 1 项全过;
- 离线 dump 复算:页高 4275 有界、无爆炸盒、页头 nav x=364 居中、药丸 span 水平排开;
- **真站重放**(守卫诞生前的双副本 dump.html,把动态副本按 React 路径 append_html
  重放):retired=5(静态 ul + 静态 quick 盒 + 旧动态副本),`header-nav-menu`/
  `header-quick-menu-box`/`header-nav-menu-home`/`我是猎头`/`首页` 全部收敛单副本;
  布局上导航 ul 529..1141 ⊘ 右锚定 quick 盒 1244..,**无叠印**(修复前动态 ul 被
  静态壳右推到 805..1417,与 quick 盒 1244..重叠 173px);
- 真机复验:`p5` 探针所有 api-c/api-wanda POST `bytes=55`(CORS 修复生效,响应真正
  到达 JS),`wait(network_idle)` 可达,静态菜单 li 被守卫清空;
- **结构级退役终验**(`p6` 探针,升级后真机):页头全部收敛单副本
  (`header-nav-menu`/`header-quick-menu-box`/`我是猎头`/`我是招聘方` 各 1);
  行文本几何 首页 513→职位 605→校园 697→海归 789→简历优化 881→关于猎聘 989,
  与右锚定 quick 盒 我是猎头 1264→我是招聘方 1372→登录/注册 1494 之间留 219px
  净空,**零叠印**(修复前动态导航被推到 853..1385 与 quick 盒 1264.. 相互叠印)。

## 四、边界(未解决,如实声明)

- React 水化断链的**具体 DOM API 缺口**未定位(生产版 React 无告警;结构级退役是
  从宿主侧收敛结果,不是水化修复);后续可对 `nextSibling`/文本节点序列等做专项对照;
- 跨源 API 现返回 55 字节业务错误体 `{"flag":0,"code":"-1400","msg":"出错了(400)!"}`
  ——请求已放行、响应已到达 JS,页面也如实渲染了站点自身的错误态;拿到真实职位
  数据需要完整登录态/Cookie 链(站点风控层),不是内核渲染问题;
- hero `<video>` 画面仍不可渲染(内核无媒体后端),只做能力缺口如实上报;
- Boa 单轮分钟级、open 重站 226s 逼近 300s 工具超时的性能边界未动;
- 登录卡 antd 表单行盒若有残留塌陷(38 轮边界),待真机复核;
- 不做「绕 WAF」;猎聘 IP/指纹级打标仍是站点风控边界。
