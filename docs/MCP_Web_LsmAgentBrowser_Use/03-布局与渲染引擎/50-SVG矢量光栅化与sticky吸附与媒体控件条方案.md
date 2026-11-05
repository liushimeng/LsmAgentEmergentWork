# 50-SVG 矢量光栅化与 sticky 吸附与媒体控件条方案

## 背景

对照 40 号文档全量控件矩阵与 49 轮收尾状态,显示/渲染系统剩余高价值缺口:

| 缺口 | 改造前 | 价值 |
| --- | --- | --- |
| **SVG 光栅化** | 只有替换元素几何 + `MediaFrameNote` 空盒占位(48 轮),图标/箭头/星标一律白板 | 最高:现代站点图标几乎全走 `<svg>`(按钮箭头、评分星、关闭叉),白板 = 截图取证信息缺失 |
| **position:sticky** | 按 `relative` 处理(无吸附,inset 被当偏移) | 高:吸顶导航/吸底栏是常见布局 |
| **video/audio controls** | 几何 + poster/占位框(48 轮),无控件条 | 中:`controls` 声明「这里有播放器」,空白无法与「页面本身空白」区分 |

CSS transition/animation(需要帧时钟基础设施)、负 z-index 压父背景、datalist/date 面板、iframe 子文档仍为后续路线(见 40 号文档)。

## 一、SVG 基础矢量光栅化(P0)

### 解析层(`src/agent/browser/svg.rs`,第 50 轮新增)

- **图形元素**:`rect`(含 rx/ry 圆角)/ `circle` / `ellipse` / `line` / `polyline` / `polygon` / `path`。
- **path 指令**:`M m L l H h V v C c S s Q q T t A a Z z`;贝塞尔按弦长自适应拍平(8..=48 段);弧线走端点参数化 → 中心参数化(SVG 规范 F.6.5)再折线逼近(≤96 段);相对/绝对与隐式重复(M 后续坐标对按 lineto)语义正确。
- **容器**:`g`(transform + 绘制属性继承)/ `defs`·`symbol`·`marker` 等(只登记 id 不直渲)/ `use`(`href`、`xlink:href` → 平移内联目标;目标是图形元素直接建形状,是 g/svg 则走子树;引用链深度墙防环)/ 嵌套 `svg`(按 x/y 平移近似)。`title/desc/metadata/style/script/foreignObject` 跳过。
- **绘制属性**:`fill`(SVG 缺省黑,`none` 不填,`currentColor` → 元素 CSS `color`)/ `stroke`(缺省 none)/ `stroke-width`(缺省 1)/ `fill-opacity`、`stroke-opacity`、`opacity`(乘法折叠)/ `fill-rule`(nonzero 缺省、evenodd)/ `stroke-linecap`(butt 缺省、round)。
- **transform**:`translate` / `scale` / `rotate(deg[,cx,cy])` / `matrix` / `skewX` / `skewY` 链,复用 `style/transform.rs::Affine` 复合;描宽按 `sqrt(|det|)` 几何平均缩放近似(非等比变换与规范有偏差,图标场景可接受)。
- **viewBox 映射**:用户坐标 → 元素内容盒;`preserveAspectRatio` 缺省 `xMidYMid meet`(等比 + 居中),`none`(拉伸),`… slice`(放大裁剪,越界由元素盒 clip);无 viewBox 时用户单位 = px。
- **防御**:形状/子路径/点数三级封顶(512/256/4096),畸形 path 不炸布局。

### 显示项与光栅化

- `render/display.rs` 新增 `DisplayKind::SvgShape`(`subpaths: Arc<Vec<(Vec<(f64,f64)>, bool)>>`,点为元素内容盒局部坐标;`fill: Option<(([u8;3],u8), evenodd)>`;`stroke`/`stroke_w`/`cap_round`)。
- `render/raster.rs::draw_svg_shape`:填充走**扫描线**(像素中心 + 2×2 超采样,evenodd 交叉计数 / nonzero 卷绕数);描边走**距离场**(复用 `dist_segment`,闭合子路径补闭合段,round cap 额外计端点圆,1px 羽化);填充先画、描边后画(SVG 语义);天然支持 transform 逆映射与 scissor 裁剪。
- 布局挂点:`LayoutCtx::emit_svg_shapes`(`layout/svg_paint.rs`),由 `emit_canvas_pixels` 公共尾钩调用 —— 块级/行内原子/flex/grid/absolute 全部替换元素路径通吃。**有形状时撤掉 `MediaFrameNote` 占位**(同 seq 通道尾部回扫);无形状保留占位(能力边界可见)。
- svg 子元素不进常规 CSS 布局(替换元素分支本就不递归子级,零冲突);`clip` = 元素内容盒(SVG 视口默认 `overflow:hidden`)。

### 边界(如实上报)

- 渐变填充(linearGradient/radialGradient)、pattern、filter、mask、clipPath、文本(text/tspan)、`<image>`、`stroke-dasharray` 不支持;CSS 作用于 svg 子元素的样式/`display:none` 不影响形状收集(仅识别 presentation 属性)。
- 非零卷绕按有向边计算,折线逼近在图标精度内与规范一致。

## 二、position:sticky 真吸附(P1)

- **核心决策:光栅期平移,不改滚动路径**。页面级滚动(`act_scroll` 无 selector)只更新 `tab.scroll_y` 推帧**不重排**;布局产物是文档坐标、光栅减滚动量。吸附若在布局期求解则每次滚动都要全量重排 —— 改为显示项携带吸附参数、光栅按滚动量现算平移,滚动零额外开销;`full_page` 截图(scroll_y=0)自动回到自然位置。
- **布局期**(`layout/sticky.rs`):块级分支进入 sticky 元素登记 `StickyMark`(seq 半开区间 + `top`/`bottom` inset + 布局期视口高 + 包含块 selector),退出时回填 `end_seq/static_y/outer_h`;`compute_full` 在显示列表终排**前**由 `sticky::resolve` 求解 `max_shift`(包含块底界 - 静态盒底,防穿透)并给区间内项打标(`fixed` 项除外)。
  - **包含块兜底**:布局器不给 html/body 建盒,sticky 挂在 body 直下(最常见形态)时回落到**页高**(初始包含块 = 视口,吸附下界 = 文档底)。
- **光栅期**(`StickyInfo::shift`):`top` 吸附 `clamp(scroll_y + inset - static_y, 0, max_shift)`(向下钉);`bottom` 吸附在盒底越过视口底-inset 时向上顶(`-max_shift..=0`);top 优先。画布位置 = 页坐标 - 滚动量 + 吸附位移(等效滚动量 = `scroll_y - shift`,与 `fixed` 的滚动豁免同一通道)。
- **行为修正**:sticky 的 inset 不再被 `relative_offset` 当成偏移(改造前 `top:10px` 会把盒子平移 10px)。
- **范围**:常规块流的视口吸附(top/bottom);元素级滚动容器内 sticky、表格行吸附、left/right 吸附、flex/grid 条目 sticky 不在本轮(矩阵如实标注)。

## 三、video/audio controls 控件条(P2)

- `video[controls]` 且盒 ≥ 48×56:底部 28px 悬浮条(半透明黑 `rgba(12,12,12,150)`)+ 白色播放三角(新 `WidgetGlyphKind::PlayTriangle`,距离场光栅)+ 进度条轨道(浅白 3px,无解码态填充恒 0)+ `0:00` 时间文案;条不占布局(悬浮绘制,`clip` = 元素盒)。
- `audio[controls]`:整盒即控件条,固有尺寸 **300×30**(`media_natural_size`,与真浏览器一致);`audio` 无 `controls` → `is_replaced_tag` 返回 false(UA display:none,不再产出 300×150 误导性空盒)。
- 挂点同 svg:`emit_canvas_pixels` 尾钩 `emit_media_controls`(`layout/media_controls.rs`)。

## 四、能力矩阵更新(40 号文档同步)

| 控件/能力 | 状态 | 说明 |
| --- | --- | --- |
| svg 图标/图形 | 🟡→✅(子集) | 形状/填充/描边/viewBox/transform/use-defs 已光栅化;渐变/文本/filter 仍缺 |
| position:sticky | 🟡→✅(视口子集) | top/bottom 吸附 + 包含块约束 + 光栅期平移(滚动零重排);容器内 sticky 仍缺 |
| video[controls]/audio[controls] | 🟡→✅ | 静态控件条(播放三角/进度/时间);audio 无 controls UA 隐藏 |

## 五、测试矩阵

| 层 | 文件 | 覆盖 |
| --- | --- | --- |
| 解析单测 | `src/agent/browser/svg_tests.rs` | path 绝对/相对/HV/弧线/贝塞尔/Z、points 负号粘连、transform 链、viewBox meet/none、use-defs、属性继承/currentColor/evenodd |
| 布局/像素单测 | 同上 | 形状进显示列表且撤占位、空 svg 保留占位、实心圆/evenodd 环/描边真实像素、sticky 参数端到端、sticky 像素随滚动吸附 |
| 媒体单测 | `src/agent/browser/layout/media_embed_tests.rs` | video controls 三角+轨道、无 controls 不画、audio 300×30、audio 无 controls 不建盒 |
| 回归 | `cargo test --lib`(2953)+ `web_form_state_roundtrip`/`web_use_engine`/`browser_canvas_screenshot` | 全绿 |

## 六、后续路线(更新)

1. CSS transition/animation:帧时钟 + 样式插值(需独立一轮,涉及推帧节奏与预算)。
2. SVG 渐变填充与 `<text>`;`<image>` 位图引用。
3. sticky 元素级滚动容器内吸附 / left-right 吸附。
4. 负 z-index 压父背景语义(当前 z_floor 近似)。
