# 19 · select_dropdown 命中鲁棒性根治与 HITL 盾区几何精化

> 版本:第 164 轮(2026-10-13)。本文档覆盖两件同源的事——自定义下拉控件
> 文本命中的可见性语义修正,与 HITL 凭证区放行的挖洞几何精化。
> 背景实测:`llaew_20261013_084832.log`(闲鱼任务,「区域→北京」卡住 8 分钟;
> 登录 HITL 盾区位置/尺寸不符)。方案全文见
> `tmpPlan/2026-10-13_MCP_WebUse下拉卡住与HITL盾区精化及Plan探索阶段方案.md`。

## 1. select_dropdown:两份代码对同一 DOM 得出相反结论

### 1.1 事故

闲鱼「区域」面板:Agent 的 `select_dropdown` 报「选项北京未找到可见元素」,
紧接着手写的 eval_js(`textContent + offsetParent` 过滤)却命中并点击成功。
**工具自带的命中逻辑比模型手写 eval_js 还弱**——模型被迫绕过工具,烧了 4 轮
无效迭代。

### 1.2 根因:可见性两侧判定互相矛盾

组件库(Ant Design 系)收起的面板普遍保留 `visibility:hidden` 的**挂载副本**
(DOM 不卸载,只用 CSS 隐藏)。对这种元素:

| 判定方式 | 结果 |
|----------|------|
| `getBoundingClientRect()` | **仍为正**(参与布局) |
| `innerText` | **恒为空串**(不渲染) |
| `textContent` / `offsetParent` | 有值 / 非 null |

旧 `pick_element_js` 用「尺寸判可见 + innerText 取文本」组合:尺寸把隐藏副本
放进候选,innerText 又让真实项/副本全部匹配失败——两个判定方向相反,
行为取决于 DOM 里哪种副本排前面,不可预测。

### 1.3 修复(全部通用化,无站点特判)

1. **三件套一致判定可见**:几何尺寸 + `getComputedStyle` 的
   `visibility/display/opacity`(继承感知)+ `textContent` 取文本。
2. **非叶子触发器**:新增 `ownText`(仅元素自身的直接文本节点),覆盖
   `<span>区域<img></span>` 这类「文本+图标」组合控件。
3. **多候选取面积最小者**:精确优先于模糊;同精确命中多个时取最深/最贴近
   文本节点的元素。
4. **自愈重试**:选项搜索最多 3 次探测(立即 → 补 `wait_ms` → 最终),
   容忍面板展开动画与「前序动作已点开面板」的初始状态;找不到触发器不再
   立即失败(面板可能已开)。
5. **失败诊断**:错误携带 `文本命中 N 个/可见 0 个(matched_but_hidden |
   no_text_match)` 与处置指引,模型一步定位而非盲试。

实现:`control/form_ctl.rs`(`select_option` / `select_dropdown` /
`pick_element_js`,自 control.rs 拆出——该文件超 1800 行硬线)。

## 2. HITL 盾区几何精化

### 2.1 跨域 iframe:从「外框挖洞」到「输入框精确挖洞」

登录框常在跨域 iframe 内(如闲鱼 `#xy-login-iframe`),主文档 JS 完全不可见,
旧行为按 iframe **外框**挖洞——洞与框内真实输入区位置/尺寸不符。

新链路 `pierce_frame_input_holes()`(unlock_zone.rs):

```
CDP DOM.getDocument(pierce=true)            # 穿透 frame tree
  → DOM.querySelectorAll(root, "input,textarea")
  → DOM.getAttributes(node)                 # 关键词过滤(跨域可用,不依赖 JS)
  → DOM.getBoxModel(node)                   # 主 frame 相对坐标(chromiumoxide 文档明确)
  → 外扩 8px,间距 ≤24px 聚类合并,≤6 条
  → "@rect:x,y,w,h" 挖洞条目
```

`enrich_allowlist_with_frame_holes()`:有精确洞时**丢弃 captcha-iframe 整外框
选择器**,8 条上限内合并(选择器优先);穿透失败返回空 → 原样保留(fail-open)。

### 2.2 选择器漂移:nth-of-type 唯一性自验

`selOf` 生成 `div:nth-of-type(2) > a:nth-of-type(6) …` 这类路径后,每次盾区
`refresh()` 都重新解析;React 重渲染/异步插入党后命中**别的元素** → 在错误
位置挖洞。自验收紧 `>= 1` → `=== 1`:歧义选择器宁可丢弃,回退整页 open
(人工操作优先于管控保留)。

### 2.3 挑战弹层⑤收窄

旧逻辑只看「fixed/absolute + z-index 最高 + 200x120~60% 视口」,顶栏/下载条/
客服浮窗会被误选。新增语义收窄:无 `dialog[open]` / `role="dialog"` /
`aria-modal="true"` 语义的纯几何层,必须含挑战内容证据(`canvas`/`iframe`,
或 ≥4 张 `img`)才可入选。

### 2.4 盾区渲染(browser_overlay.rs)

- **`@rect:` 绝对视口挖洞**:白名单条目以 `@rect:` 开头时跳过选择器解析直接
  入洞(带有限性校验与视口裁剪);随 scroll/resize 由既有 rAF 节流 schedule
  重算。
- **子 frame 一律不加盾**:`allow` 分支顶部 `window.top !== window` 即
  return(fail-open)。白名单语义定义在主文档坐标空间,子 frame 按自身坐标系
  罩盾只会把主文档挖好的洞从内部锁死。主文档零命中同样不加盾(旧行为是
  满屏深色罩——「盾区位置不对」的另一种表现)。
- **布局位移重建**:补 `load` 事件 + 文档级 `ResizeObserver` 触发器——
  图片/字体加载、同域 iframe 撑高等纯布局变化不触发 MutationObserver,
  但会移动白名单元素的矩形。

### 2.5 接线一致性

`request_human`(自动档)与 `inspect(info=blockers)` 统一走
`probe_credential_zones_kinds` + `enrich_allowlist_with_frame_holes`,
两处给出的 `credential_zones` / 实际放行生效的白名单完全一致
(`data.credential_zones` 可直接回传 `allow_selectors`,含 `@rect` 条目)。

## 3. 行为变更对账

| 场景 | 旧行为 | 新行为 |
|------|--------|--------|
| select_dropdown 面板已开 | 触发器重找,可能误点收合 | 找不到触发器不失败,选项搜索自愈重试 |
| visibility:hidden 挂载副本 | 尺寸判可见/innerText 漏配,行为不可预测 | 计算样式判隐藏,textContent 命中但过滤 |
| 跨域登录 iframe | 整外框挖洞,位置/尺寸不符 | 输入框精确矩形洞(@rect),穿透失败回退外框 |
| 歧义 nth-of-type 选择器 | 多命中也接受,漂移后挖错洞 | 唯一性自验,歧义丢弃回退整页 open |
| 子 frame 白名单非零命中 | 按子 frame 坐标系罩盾(错位) | 一律不加盾(fail-open) |
| 白名单主文档零命中 | 满屏深色罩 | 不加盾(fail-open) |
| 图片/字体加载位移 | 盾区不重算 | load + ResizeObserver 触发重建 |

## 4. 杀开关与排障

- 凭证区探测整体关闭:`LAEW_WEB_HITL_SCOPE=off`(回退整页 open,第 143 轮行为)。
- `@rect` 条目出现在 `data.page_unlock.allow_selectors` / `data.credential_zones`
  即表示走了跨 frame 精确洞;出现整 iframe 选择器表示穿透失败(fail-open)。
- select_dropdown 报「命中但隐藏」:面板收合的挂载副本,先点击触发器展开再重试。
