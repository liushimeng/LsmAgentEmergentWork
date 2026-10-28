# 30 - 猎聘搜索页脚本启动链与有头窗口可见性修复

> 对应方案:`tmpPlan/20261028_084931_猎聘搜索页动态内容与浏览器观测可信度修复.md`
> 场景:猎聘搜索页在 LsmAgentBrowser 中只有页头/静态页脚,Chrome 同 URL 有 40 个职位卡片;有头查看器进程存在但用户未看到窗口。

## 一、诊断顺序与证据

1. `inspect(console)` 首先证明 `common.*.js` 超过固定 2 MiB,被网络层直接取消;不是 CSS/光栅化问题。
2. 提高预算后,网络台账确认 `common.js` 实际大小 3,301,380 字节,webpack 继续加载 9 个 chunk;但 `#lp-search-job-box` 仍为空,说明还有启动链缺口。
3. webpack 入口是 `Promise.all([...]).then(启动模块15281)`。通过向 `webpackChunkfe_www_pc` 推入诊断 runtime 捕获 `__webpack_require__`,再直接执行启动模块,才把 Promise 链遮蔽的真实异常暴露出来。
4. 异常逐层推进:`console.assert` 缺失 → SVG `setAttributeNS` 缺失 → React effect 查询同批动态节点得到 `null`。这三处都是通用浏览器语义缺口,不应做站点特判。
5. 用户要求停止真站测试后,不再继续访问站点;最终职位卡片恢复情况不在本文中宣称,剩余验收用离线回归完成。

## 二、脚本资源预算

`script/mod.rs` 的经典脚本默认单文件上限从 2 MiB 提到 8 MiB,新增:

```bash
LAEW_WEB_MAX_SCRIPT_BYTES=8388608
```

- 首屏外链脚本预取、缓存读取、动态 `<script src>` 线程内装载共用该预算。
- 普通 fetch/XHR 仍保留原响应上限,不随脚本预算放大。
- 集成测试构造 >2 MiB 的合法公共脚本,断言其中 DOM 写入真实发生,避免只检查「不再报警」。

## 三、JS/DOM 兼容修复

### 1. `console.assert`

浏览器断言失败只写控制台,不抛异常。PRELUDE 现按该语义实现;true 不记录,false 记录 `Assertion failed` 与附加参数。缺失该方法会让站点初始化在无关紧要的自检处直接中止。

### 2. 命名空间属性 API

静态快照节点和动态节点统一支持:

- `setAttributeNS` / `getAttributeNS`
- `hasAttributeNS` / `removeAttributeNS`

当前内核没有完整 XML 命名空间模型,按 DOM 限定名(如 `xlink:href`)读写属性;该边界足以让 React/SVG 完成挂载,不宣称已实现完整 NS 语义。

### 3. 动态 DOM 同批读写一致性

旧模型只查上一轮宿主快照,动态元素的 `querySelector(All)` 甚至固定返回空。React commit 后的 effect 在同一批 JS 中读取刚创建节点时,宿主尚未回写,于是出现「节点已创建但查询为 null」。

现行为:

- `document` 查询 = 静态快照结果 + 已挂载且尚未回快照的动态结果;
- 静态元素作用域查询包含它新挂载的动态子树;
- 动态元素可查询自己的动态后代,detached 子树也可查;
- 动态节点被快照收养后只走静态结果,不重复返回;
- `remove()` 清理父链/mounted 状态,同时向宿主发 `remove_node`;重新 append 可再次挂载,不产生重复节点。

回归同时覆盖:同批 document/root/scoped 查询、未布局几何为 0 而非 null、remove 后消失、跨快照去重。

## 四、观测结果可信度

### 1. 阻断检测不再匹配裸「风控」

招聘页正常包含「风控算法/风控工程师」。`inspect(blockers)` 现只匹配带明确访问/拦截语义的短语(如「异常访问」「风险拦截」),避免把正常业务内容误判为必须人工介入的风控页。

### 2. 复用页面返回真实 mode

`new_page` 复用分支曾把 `OpenOutcome.mode` 硬编码为 `hidden`,即使既有页是 headed。现读取 `tab.mode`,窗口状态与工具返回不再互相矛盾。

### 3. WebGL 申请不遮蔽脚本错误

安全/指纹脚本申请 WebGL 不等于页面主画面依赖 WebGL。`page_health` 优先报告脚本执行错误;无脚本错误时才提示 WebGL 画布能力边界,措辞从确定依赖改为「申请过、对应画布可能无像素」。

## 五、有头窗口可见性

进程表中出现 `laew __browser-view` 不能证明用户看到了窗口。macOS 旧行为只 `makeKeyAndOrderFront`,不激活应用,窗口容易被 Chrome/终端遮住。

现首次启动默认 `activateIgnoringOtherApps(true)`,把 headed 窗口带到前台;需要旧行为时:

```bash
LAEW_BROWSER_VIEW_ACTIVATE=off
```

本地独立帧诊断不访问站点:推送 800×600 PNG,抓屏确认标题、后退/前进/刷新/首页、地址栏和页面帧都显示;修改后再次抓屏确认窗口成为当前 `laew` 应用前台窗口。诊断裁剪图为 `/tmp/lsm_viewer_window.png`(临时产物,不入库)。

## 六、回归与边界

- 浏览器内核相关测试 29 项通过;
- 脚本运行时测试 71 项通过;
- `web_use_engine` 22 项通过;
- release 构建通过。

macOS 已完成本地窗口抓屏验证;Windows/Linux 未在本轮环境实测。猎聘最终采集任务未继续执行,不能把上述兼容性修复表述为「20-100 条岗位任务已完成」。
