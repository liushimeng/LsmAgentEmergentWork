# 55-SVG 渐变 / 文本 / 位图引用与媒体 poster 显示补全方案

## 背景

`50-SVG矢量光栅化与sticky吸附与媒体控件条方案.md` 落地了 SVG 矢量光栅化子集
（rect/circle/ellipse/line/polyline/polygon/path + fill/stroke/transform/viewBox），
并在篇末如实标注边界：**渐变 / 文本 / `<image>` / filter 仍缺**。
`40-Web控件全量能力盘点与增量实施方案.md` 的「后续路线」同样把这些列为待办。

本轮把其中三块补成**真实像素**，并顺带修一个「已下载却不显示」的媒体缺口：

| 缺口 | 修复前（代码事实） | 修复后 |
| --- | --- | --- |
| SVG 渐变 | `svg.rs` 把 `lineargradient`/`radialgradient` 与 `defs` 一起跳过；`paint_value` 见到 `url(` 直接返回 none | 线性/径向渐变填充与描边，含 `stop`/`href` 继承/`gradientUnits`/`gradientTransform`/`spreadMethod` |
| SVG 文本 | `shape_subpaths` 只认 7 种图形标签，`text` 落到 `_ => None` | `<text>`/`<tspan>` 文本 run 进显示列表，复用既有字形光栅 |
| SVG `<image>` | `"image" => {}` 直接丢弃 | `href` 进预取链 + 布局期解析 + `preserveAspectRatio` 三态绘制 |
| 媒体 poster | `video[poster]` 只进预取链与固有尺寸；`emit_element` 一律画 `MediaFrameNote` 空盒 | 资源已解码时画真实 `Image` 并撤占位；未加载仍保留占位 |

## 一、SVG 渐变

### 1.1 支持子集

- `<linearGradient>` / `<radialGradient>` + `<stop>`。
- `id` / `href`|`xlink:href`（继承父渐变缺省属性与色标）/ `gradientUnits`
  （`objectBoundingBox` 缺省、`userSpaceOnUse`）/ `gradientTransform` /
  `spreadMethod`（`pad` 缺省 / `reflect` / `repeat`）。
- 线性 `x1`/`y1`/`x2`/`y2`（缺省 `0% 0% 100% 0%`）；
  径向 `cx`/`cy`/`r`/`fx`/`fy`（缺省 `50% 50% 50%`，`fx`/`fy` 缺省 = `cx`/`cy`）。
- `<stop>`：`offset`（数字或 `%`）/ `stop-color`（含 `currentColor`）/
  `stop-opacity` / `style="stop-color:…;stop-opacity:…"`。
- 引用：`fill="url(#id)"` / `stroke="url(#id)"`，支持 `url(#id) fallbackColor` 兜底色。

### 1.2 落点

| 文件 | 改动 |
| --- | --- |
| `src/agent/browser/svg.rs` | 新增 `SpreadMethod`/`GradientStop`/`GradientPaint`/`SvgPaint`；`collect_gradients()` 解析 defs 并处理 `href` 继承；`paint_value` 返回 `Option<Option<SvgPaint>>`；`SvgState.fill/stroke` 改 `SvgPaint` |
| `src/agent/browser/layout/svg_paint.rs` | `DisplayKind::SvgShape` 的 `fill`/`stroke` 改带 `SvgPaint` |
| `src/agent/browser/render/display.rs` | `DisplayKind::SvgShape` 字段类型调整 |
| `src/agent/browser/render/raster.rs` | `draw_svg_shape` 逐采样点求 `t`（`svg_gradient_t`）+ `spreadMethod` + 色标插值（`svg_gradient_color`） |

### 1.3 关键语义决策

1. **objectBoundingBox**（缺省）：梯度坐标是形状 bbox 的单位方形，
   `p_local = T_bbox(gradientTransform(p))`，
   `T_bbox = translate(bbox.x0,bbox.y0) ∘ scale(bbox.w,bbox.h)`。
   光栅期把局部点归一进单位方形，再用 `inv`（`gradientTransform` 的逆）
   变回渐变空间，因此**径向渐变在 `gradientTransform` 带旋转/缩放时仍按椭圆正确求值**
   （不是「标量半径 + 圆」近似）。
2. **userSpaceOnUse**：梯度坐标视为引用元素的根用户空间，
   在 `collect_nodes` 里统一施加 `viewbox_map ∘ tf ∘ gradientTransform`
   （`tf` 取引用元素自身的 `SvgState.tf`，与形状点同一变换链）。
   径向半径按变换的几何平均缩放 `sqrt(|det|)` 预解。
3. **spreadMethod**：`pad` 钳位、`reflect` 镜像（`t.abs() % 2`）、
   `repeat` 平铺（`t % 1`），正负两个方向都生效。
4. **色标 offset 非递增**：钳到前一个 offset（不重排），避免反相渐变。
5. **alpha**：`stop-opacity` × `fill-opacity`/`stroke-opacity` × 组 `opacity`
   统一在 `scale_paint_alpha` 折叠；渐变按「先乘进每个 stop」处理。
6. **多采样点颜色平均**：`draw_svg_shape` 原本对每个像素取 4 个亚采样点
   只做覆盖率平均；渐变下改为**按落在形内的采样点数**平均颜色与 alpha
   （分母是形内采样点数，不是固定 4），避免边缘像素被稀释。

### 1.4 边界（如实上报）

- 径向渐变焦点 `fx/fy` 偏移**不建模**：按圆心距离近似。多数站点
  `fx=fy=中心`，无差异；显式写了 focal 的渐变位置会偏。
- `pattern` / `filter` / `mask` / `clipPath` / `marker` 仍不渲染。
- `color-interpolation`（线性 RGB 与 sRGB）不实现，统一 sRGB。
- CSS 样式作用于 svg 子元素（`svg * { fill: … }`）仍不支持，只认呈现属性与 `style`。

## 二、SVG `<text>` / `<tspan>`

### 2.1 支持子集

- `<text>` / `<tspan>`：`x`/`y`（数字流，取首值）、`dx`/`dy`（数字流，累加）、
  `font-size`、`font-weight`（`bold` 或 ≥600）、`fill`（含 `none`/`currentColor`）、
  `text-anchor`（`start`/`middle`/`end`）、`dominant-baseline`
  （`middle`/`central` 走「y = 字形盒垂直中心」近似，缺省 y = 字母基线）。
- 字符数据来自 `text_contents()`（含 tspan 拼接），空白按 SVG 缺省规则折叠。

### 2.2 落点

| 文件 | 改动 |
| --- | --- |
| `src/agent/browser/svg.rs` | 新增 `SvgText` + `TextState`；`emit_text_runs`/`push_text_children`/`normalize_text`；`walk_children` 的 `text` 分支 |
| `src/agent/browser/layout/svg_paint.rs` | `SvgNode::Text` → `DisplayKind::Text`（`text-anchor` 用 `FontSet::text_width` 算偏移；`dominant-baseline:middle` 时把 `y` 折成行顶） |

### 2.3 关键语义决策

1. **绘制通道复用**：发射既有 `DisplayKind::Text`（raster 走 `draw_text`），
   不新建通道，避免第二套字形光栅。
2. **文档序**：`collect_nodes` 用 `Vec<SvgNode>`（`Shape`/`Text`/`Image`）
   单表输出，文本与图形的上下叠放关系与 DOM 一致。
3. **变换链**：run 坐标先按 `<text>` 元素自身的 `st.tf` 搬到根用户空间，
   再随 `viewbox_map` 进内容盒局部坐标 —— 与形状点同一套变换。
4. **基色**：取继承的 `SvgState.fill`（与形状同源），`fill:none` 不画；
   渐变文字不在子集内，退回 CSS 当前色。
5. **tspan**：每个带直接文本的 `<tspan>` 产出一条 run，嵌套 tspan 递归；
   `x`/`y` 覆盖、`dx`/`dy` 累加。

### 2.4 边界

- 多行 tspan（每行一个 x/y）按「每 tspan 一条 run」处理，**行距不自动加**；
  同一 tspan 内的换行被空白折叠规则压成空格。
- `font-family` 只影响选字体链，不加载网络字体；`font-style:italic` 不实现。
- `textLength`/ `lengthAdjust` 不实现。

## 三、SVG `<image>`

- `href` / `xlink:href` 进 `images::collect_image_urls` 预取链（与 `<img>`/poster 同链）。
- 布局期按 `base_url` 解析，`ImageSizes` 已解码才发射 `DisplayKind::Image`；
  `preserveAspectRatio`：缺省 `xMidYMid meet` → `object-fit:contain`，
  含 `slice` → `cover`，`none` → `fill`。
- **资源未加载时保留 `MediaFrameNote` 占位**：`emit_svg_shapes` 先判断
  「是否有可绘制节点」，只有形状/文本或已解码的 `<image>` 才算数。

## 四、媒体 poster

- `video[poster]` / `object[data]` / `embed[src]`：资源已解码时发射真实
  `Image`（按 `object-fit` 缺省 `fill`），并撤掉本元素的 `MediaFrameNote`。
- **发射顺序**：`emit_canvas_pixels` 里 poster 必须先于 `emit_media_controls`，
  控件条走 `next_seq()`（seq 更大）自然叠在海报之上。
- 仍未解码/非图片资源时保留占位（「能力边界可见」语义不变）。

## 五、验证

```bash
cargo test --lib svg_tests          # 33 例:渐变/文本/image/poster 解析 + 像素
cargo test --lib                    # 2976 例全绿
cargo test --test web_use_engine --test web_form_state_roundtrip \
          --test web_human_input --test browser_canvas_screenshot \
          --test browser_mode_env
./rebuild_restart_app.sh
```

新增像素回归（`src/agent/browser/svg_tests.rs`）：
线性渐变端点色、径向中心/边缘、`userSpaceOnUse` 映射、`spreadMethod:repeat`
周期、`href` 继承、缺引用兜底色、渐变描边、文本进显示列表与落像素、
文本继承 `fill` 与组变换、`svg image` 预取/绘制/未加载保留占位、
`video[poster]` 绘制并撤占位。

## 六、后续路线（本轮后更新）

1. ~~select 原生弹层/multiple、date/color/range 专用 UI~~（41/44 轮已落地）。
2. ~~dialog top-layer/::backdrop/焦点圈禁~~（41/42 轮已落地 top-layer + backdrop；
   **焦点圈禁仍缺**）。
3. ~~SVG 渐变 / 文本 / `<image>`~~（本轮完成）；**`filter` 仍缺**。
4. Shadow DOM 与动画帧同步；sticky 容器内吸附仍缺。
5. iframe 子文档隔离仍非目标。
