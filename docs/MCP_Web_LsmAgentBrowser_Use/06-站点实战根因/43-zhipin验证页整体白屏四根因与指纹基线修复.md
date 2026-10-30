# 43 · zhipin 验证页「整体白屏」四根因与指纹基线修复

> 日期:2026-10-30
> 方案:`tmpPlan/20261030_092731_zhipin验证页白屏渲染与GeeTest弹层链修复.md`
> 场景:`Dir_LsmAgentBrowser_zhipin_01` 真机任务(BOSS 直聘采集,`laew --debug -f`)。
> 入口 302 → verify.html → `request_human(captcha)` 移交人工 → 用户在查看器看到
> 「整页白屏 + 只有『安全验证』卡片」,83 秒取消任务。用户判定渲染完全不达标。

## 一、取证方法(四个临时真站探针,tests/tmp_*.rs,验完即删)

| 探针 | 手段 | 关键证据 |
| ---- | ---- | ---- |
| #1 | 截图 + `LAEW_DUMP_HTML` + `LAEW_DUMP_DISPLAY` | 截图 98% 纯白;显示列表 18 项里验证卡片(标题/提示/渐变按钮/验证按钮/版权)**都在**——「布局有内容/渲染近全白」定性为**底色缺失**而非内容缺失 |
| #2 | `inspect(elements)` 几何 + 点击 `#rowCode` | holder 440×44 布局正常;点击后截图**字节级未变**——GeeTest 弹层未出现 |
| #3 | 全量 console + network | `reset.php` ×73 全 200;`get.php` ×4 携完整 `w=` 指纹;本轮无 `[cbtrap]` 回调异常 |
| #4 | `eval_js` 钩 `__lsmFetchNative` 抓 JSONP 响应体 | **reset.php 响应全部 `status:success`**——服务端并未拒绝,循环驱动在厂商 fullpage 内部;另验出指纹面三缺口(见根因 4) |

**截图链路结论**:截图与显示列表逐项吻合、`full_page` 正常(1920×1162),截图功能本身无 bug。

## 二、根因与修复(全部通用,无站点特判)

### 根因 1(渲染):html/body 根画布背景从不绘制 → 整页纯白

- `layout/mod.rs::layout_document` 从 body 子元素起排,html/body 不建盒,两者
  `background` 从不发射 → 底色永远是渲染层白底。zhipin verify 页正是
  `body{background:var(--bg-page)}`(#f5f7fa):Chrome 淡蓝灰底 + 白卡片,本内核纯白
  页 + 白卡片 = 「整体白屏」直接构成。**全站通用缺口**(多数站点背景写在 body 上)。
- **修复**:CSS 2.1 §14.2 根背景传播——html 有背景用 html 的,否则 body 的上传播;
  以 `z=i64::MIN` 画布背景项铺 `max(页高,视口高)`,位于一切内容(含负 z-index)之下。
  html+body 同时设置时 body 自身背景仍不发射(body 无盒,与修复前一致)。
- **真站验证**:修复后截图四角像素 = `rgb(245,247,250)`(即 `--bg-page`),卡片区白色,
  与 Chrome 观感一致。

### 根因 2(工具层):inspect/control 参数透传白名单缺键,宣传的参数被静默丢弃

- `inspect(elements, list=true)`:`normalize_inspect_params` 白名单无 `list` → 永远回退
  单元素模式(hint 文案却宣传 list=true)。探针 #2 实测只拿到 1 个元素。
- `control(eval_js, expression=…)`:act_eval 只从 `params` 读
  `expression/script/function/js/code`,而 `normalize_params` 顶层白名单不含 → 报
  「eval_js 需要 expression」。
- **修复**:inspect 白名单补 `list`;control 白名单补
  `expression/script/function/js/code/await_promise`。模型按工具描述顶层传参是常态,
  静默丢弃会让 Agent 白费多轮往返。

### 根因 3(测试基建):mcp_window_use doom_loop 三测试共享全局态并行互踩

- `doom_loop_check/reset` 是进程级全局;三个 `#[test]` 并行跑,reset/check 互相踩。
  本轮新增 5 个布局测试改变调度时序后必现竞态红(单测过、全量挂)。
- **修复**:三场景合并为一个顺序测试函数,天然串行。

### 根因 4(指纹基线):默认 UA 自曝爬虫 + navigator 身份三缺口

探针 #5(eval_js 能力面)实测修复前:

| 项 | 修复前 | Chrome 参照 |
| -- | ------ | ----------- |
| HTTP UA / navigator.userAgent | `Mozilla/5.0 (compatible; LsmAgentBrowser/0.1; +https://localhost/laew)` | 桌面 Chrome UA |
| navigator.platform | `"LsmAgentBrowser"` | `MacIntel`/`Win32`/`Linux x86_64` |
| navigator.plugins / mimeTypes | **缺失**(undefined) | 5 插件 / 2 MIME(PDF) |
| navigator.deviceMemory | undefined | 8 |

- `DEFAULT_UA` 自曝爬虫身份且与 `navigator.userAgent` 同源透传,厂商指纹按
  「非常见浏览器 UA」直接降权——与 Lsm 内核「反-反爬专用」定位相悖。
- **修复**:
  1. `DEFAULT_UA` 按宿主 OS 合成桌面 Chrome UA(`cfg!` 三平台);需要机器人礼貌标识的
     场合由 `open(user_agent=...)` 显式覆盖;
  2. prelude 新增 `__lsmApplyUaIdentity(ua)`:UA 单一真源派生 platform/appVersion/vendor
     (Win→Win32、Mac→MacIntel、Linux→Linux x86_64;appVersion=UA 去 `Mozilla/` 前缀);
     meta 应用与 `__lsmSetUa` 均改走它;
  3. `__lsmBuildPdfPlugins()`:Chrome 真实形状的 5 插件 × 2 MIME 数组(item/namedItem/
     enabledPlugin/`Symbol.toStringTag` 齐备);
  4. navigator 补 `vendor/appCodeName/product/productSub/deviceMemory`。
- **验证**:修复后页面内 `plugins.length=5`、`mimeTypes.length=2`、`platform=MacIntel`、
  `deviceMemory=8`。

## 三、GeeTest reset 循环:本轮新证据与边界

- 修复指纹基线后 **reset 循环仍持续**(reset.php 依旧每 5-6s 一次、响应全 success),
  但循环驱动不变:厂商 fullpage 内部拿到新 challenge 后不调 get.php/ajax.php 直接
  重置,错误被厂商内部 catch 吞掉(console 干净、无 `[cbtrap]`)。
- 结合 37 号文档三形态定性:**`w=` 密文内的深层信号(canvas 光栅哈希/WebGL 串/字体枚举
  /时序)与本内核差异仍会被风控评分**。指纹保真度专项(GPU 后端、canvas 哈希、
  WebGL 参数)是既记档的后续路线,本轮只把「身份标识层」做实。
- 点选题型(canvas/WebGL 画题面)在 Lsm 内核上人工也无法完成,继续按双浏览器分工走
  `MCP_Web_Chrome_Use`。

## 四、离线回归

- `cargo test --lib`:2717 全过(新增根画布背景 ×5:body 上传播/铺满视口且位于一切
  之下/html 优先/均无不发射/渐变传播;doom_loop 三场景合并后 -2 净计数)。
- `cargo test --test web_use_engine`:26/26。

## 五、已知残余(如实记档)

- SVG 内联图标不渲染(装饰性,既知边界);`sprite.1.5.8.png` 等 404 是站点自身资源缺失;
- 验证按钮行内 160×1 细条是挂件内部状态条几何,厂商链路修复后自然消解;
- `navigator.userAgentData` 高熵提示未实现(Chromium 专属,部分风控会探测,后续专项)。
