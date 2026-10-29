# 32 - 百度地图显示不全三根因与非 HTML 主文档渲染修复

> 对应方案:`tmpPlan/20261028_130342_百度地图显示不全三根因与非HTML文档渲染修复.md`
> 场景:第 175 轮真机(`Dir_LsmAgentBrowser_baidu_03`,`--debug -f` 百度地图路线规划)。
> 主线任务本身在 hidden Chrome 上 7 工作流全过(扫码登录→路线→出行计划-01.md),
> 但用户可见的 Lsm 查看器地图只画左上一角 + 二维码页全黑 —— 本轮修显示层。

## 一、根因与证据(全部真机探针实测)

### 1. 初始包含块硬编码 UA body margin(布局层)

- `layout/mod.rs::compute_full` 把 `y=8`、`content_width=width-16`、`children_x=8`、
  `height=y+8` **硬编码**——与 ComputedStyle 的真实 body margin 脱钩;
- 站点 reset `body{margin:0}` 后(样式引擎正确返回 0px),布局仍偏 8px、收窄 16px:
  `#MapHolder=(8,27,1904×1080)`,`map.getSize()=1904×1080`;
- **修复**:四处处改读 `body_style.margin_*`;UA 默认 8px 仍在样式层(`ua_default`),
  无样式页面观感不变,reset 后自然全幅。修复后 `#MapHolder=(0,3,1920×1080)`。

### 2. 空行内元素产生幽灵行盒(布局层)

- body 首子 DIV(仅 `<a name="script…"></a>`)被量出 1904×17:
  `inline_box_worthy` 对一切 `<a>` 建盒,`inline_box_size` 给空元素
  `w=max(0,40)`/`h=max(font×1.45,16)` 幻影尺寸,`row.register` 再推进游标;
- Chrome 语义:空行内元素 0×0、不产生行盒;第 174 轮注释明言近似盒
  「不影响流」,实现违背意图;
- **修复**:新增 `has_structural_inline_content`(纯结构递归,非空文本/替换控件类即内容);
  空元素量 0×0、不登记行占用不推进游标,**盒照常发射保住 id/标签寻址**;
  表单控件(input/select/button/canvas…)与带显式尺寸元素不受空判定影响(void 元素无子节点)。

### 3. 非 HTML 主文档黑屏(网络→文档层)

- 导航 `passport.baidu.com/v2/api/qrcode?…`(Content-Type: image/png)后全黑:
  `open`/`navigate` 对响应一律 `resp.text()`→HTML 解析,PNG 字节流→空 DOM;
- Agent「把可见 Lsm 窗口当二维码展示板」的路径被阻断(HITL 扫码只能靠 Preview 弹窗);
- **修复**:`synthesize_non_html_document`——`image/*`(≤4MB)**data: 内嵌**(一次性资源
  二次取回会变样)flex 居中展示;`text/plain`/`json` 等宽 `<pre>`(HTML 转义);
  其余二进制类型说明页;HTML/XML/空 CT 返回 None 走原路径。`open`+`navigate` 双接入。

### 4. PRELUDE 兼容缺口四件(脚本层)

| 缺口 | 现象 | 修复 |
| ---- | ---- | ---- |
| `el.offsetParent` | 恒 undefined,瓦片链路探测走一层就断 | getter:沿 `parentNode` 上行取首个定位祖先,无则 body;body/html/fixed→null(Chrome 近似) |
| `getComputedStyle(el).margin/.padding` | undefined,探测脚本误判 | `to_map` 序列化补四值简写 |
| `document.styleSheets` | `.length` 直接 TypeError,整段探测脚本作废 | 最小空 StyleSheetList(length 0),真枚举另立项 |
| `getComputedStyle(el).width/height` 为 auto | 真浏览器返回 used value | 有快照 rect 且非 inline 时回填 px |

## 二、修复效果与诚实边界

- 容器几何全对齐:`#MapHolder (0,3,1920×1080)`、`map.getSize()=1920×1080`、
  `cs.height=1080px`、幽灵条带 17px→≈0;
- **但下半屏瓦片空白的主症状仍在**:瓦片恒 8×5=40 格(行列数=ceil(1920/256)×ceil(1080/256)
  **正确**),栅格中心恒 (418,7) 而非 (960,540),整体偏移 ≈(-542,-533);
  resize/checkResize/点击补齐均不重铺;`#platform`(瓦片 offsetParent)无显式尺寸,
  我们量 40×3(幻影下限)但瓦片按 left/top 绝对定位,platform 尺寸不影响瓦片位置(已排除);
- **Chrome 无法对照**:Chrome 有 WebGL,百度地图走 canvas 底图(同页仅 9 张 img、零瓦片);
  栅格瓦片是无 WebGL 时的**降级路径**,只在 Lsm 内核走到;
- **下一轮方向**:JS hook 抓降级路径初始化时刻读的 API 返回值
  (about:blank 注入 hook + `--disable-webgl` 的 Chrome 实例对照)。

## 三、通用性说明

- 五项修复全部为通用浏览器语义,**零站点特判**;
- 新增单测 7 例:`body_margin重置后内容全幅从原点排布`/`无样式时body默认margin为样式真值`/
  `空锚点不产生幽灵行高`/`带文本锚点照常建盒`/`css_map含margin与padding简写`/
  `非html文档合成展示页` + 既有 `行内元素自身建盒` 回归;
- 全量 `cargo test --lib` 2605 通过。

## 四、架构层观察(不在本轮修复,记录决策)

- 主线任务在 **hidden Chrome** 完成(用户全程不可见),可见的只有 Lsm 查看器;
  「像素级底图改 Chrome」的工具 hint 与「用户要看到浏览器」的诉求冲突——
  Chrome 形态是否随任务诉求联动 headed,单独立项。
