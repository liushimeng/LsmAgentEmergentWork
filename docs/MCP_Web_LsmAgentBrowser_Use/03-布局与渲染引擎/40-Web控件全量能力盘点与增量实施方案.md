# 40-Web 控件全量能力盘点与增量实施方案

## 背景

LsmAgentBrowser 已有「HTML 解析 → CSS 计算 → 布局 → 显示列表 → 软件光栅化」主链，但 Web 控件能力分散在多篇历史文档中，且存在端到端断点：`TabState.form_values` 的运行时值已经用于 inspect/表单提交，却没有稳定进入布局与截图，导致「操作成功但页面仍显示旧值 / checkbox 无勾 / select 仍流出全部 option」等显示异常。

本轮目标不是一次性复刻完整 Web 标准，而是：

1. 给出**全量控件能力矩阵**，明确 已实现 / 部分实现 / 本轮新增 / 非目标，防止重复开发。
2. 打通**表单运行时状态 → 样式/布局 → 显示列表 → 像素 → 交互重排**的垂直闭环。
3. 补齐高价值语义切片：details/dialog 显隐、Canvas 固有尺寸、列表 marker、语义属性保真、基础表格格式化上下文。

## 状态口径

| 标记 | 含义 |
| --- | --- |
| ✅ | 已实现，且有既有测试/稳定边界 |
| 🆕 | 本轮新增或本轮补齐，且有本轮测试 |
| 🟡 | 部分实现/近似实现，必须同时读「边界」列 |
| ⛔ | 本轮明确非目标或未实现 |

判定一个控件「已实现」至少核对：DOM/inspect、几何布局、像素绘制、Agent 交互、默认行为、JS 状态同步、自动化测试。仅解析出 DOM 或仅返回动作成功不算显示已实现。

## 一、核心落点（本轮代码）

| 能力 | 落点 |
| --- | --- |
| 运行时控件状态覆盖 | `src/agent/browser/mod.rs::TabState.form_values/widget_open`；`src/agent/browser/document.rs::Document::apply_widget_state` |
| inspect/JS/布局/截图同源 | `TabState::document_with_state`，api/inspect/view 重排点统一走它 |
| 控件图元 | `src/agent/browser/render/display.rs::WidgetGlyphKind`、`render/raster.rs::draw_widget_glyph` |
| 表单控件绘制 | `src/agent/browser/layout/mod.rs::emit_form_control_text` 及单行/多行/meter 辅助 |
| 语义显隐 | `src/agent/browser/style/mod.rs::computed` 的 details/dialog 硬约束；`style/values.rs` 的 option/optgroup/datalist UA none |
| 基础表格 | `src/agent/browser/layout/table.rs`，由 `layout_children`/`measure_content` 在 `display:table` 分发 |
| 交互重排 | `src/agent/browser/api.rs::relayout_and_push`；input/clear/select/checkbox/radio/summary 操作后必重排 |
| JS 事件保底 | `js_dispatch_bounded`：click/input/change 最多等 2s，线程忙时保底推进原生默认动作 |

运行时状态不回写 `tab.html`，只在临时解析树上覆盖，避免把用户输入污染源 HTML；`form_values` 的 key 仍是 `Document::stable_selector`。

## 二、全量 Web 控件矩阵

### 1. 表单交互控件

| 控件 | DOM/交互 | 布局/像素 | 状态 | 边界 |
| --- | --- | --- | --- | --- |
| 文本框/搜索/邮箱/电话/URL | value/placeholder/输入/清空 | 单行文本、placeholder 灰字 | 🆕 | 无光标/选区/编辑历史 |
| 密码框 | value 运行时同步 | `•` 掩码 | 🆕 | 无明文切换 |
| 数字输入框 | value/min/max/step 属性保真 | 按文本框绘制 | 🟡 | 无增减按钮、步进校验 |
| 复选框/单选框 | checked 运行时同步、点击切换、radio 同 form/name 互斥 | 几何勾/圆点 | 🆕 | 无 indeterminate、无自定义皮肤 |
| 下拉选择器 | select_option/select_dropdown、selected option | 只画当前 label + 下拉箭头 | 🆕 | 无原生弹层、multiple/size 列表 |
| 多行文本框 | value/placeholder | 按盒宽多行换行 | 🆕 | 无滚动条、rows/cols 精确语义、光标 |
| 按钮/提交/重置 | JS click 先行，preventDefault 尊重；表单提交 | value/默认文案 | 🆕 | 无按压/焦点态、无 CSS :active |
| 进度条 | value/max | 轨道 + 比例填充 | 🆕 | 无不确定态动画 |
| 度量条 | value/min/max/low/high/optimum 属性保真 | 轨道 + 比例填充 | 🆕 | 不按 low/high/optimum 分段着色 |
| 字段集/字段标题 | DOM/accessible name | block/inline 几何 | 🟡 | 无 fieldset disabled 继承、legend 嵌入边框 |
| 标签 | label[for] 参与 accessible name | 可寻址行内盒 | 🟡 | 点击不聚焦/激活关联控件 |
| 数据联想框 | datalist/option 存在于 DOM | 默认 `display:none` | 🆕 | 无 input list 联想弹层 |
| 日期/时间选择器 | value/属性保真、可输入 | 文本框式几何 | 🟡 | 无日历/时间面板 |
| 文件选择器 | act_upload 记录路径到 form_values | 按钮/文本框几何 | 🟡 | 不读真实文件系统、不显示文件列表 |
| 滑块/颜色选择器 | min/max/step/value 属性保真 | 通用盒 | 🟡 | 无滑块轨道/拇指、颜色 swatch |
| 输出框/output | DOM/文本 | inline 盒 | 🟡 | 无 for 关联即时计算 |
| fieldset/legend | ✅（第 48 轮） | fieldset UA 边框+padding；legend 块级化；legend 嵌边为近似 |
| label | 🟡 | 可寻址、参与 accessible name；点击不转发到关联控件 |

### 2. 语义化展示控件

| 控件 | 状态 | 说明/边界 |
| --- | --- | --- |
| h1~h6、p、行内容器、超链接、strong/em | ✅ | 块/行内排版、文本换行、链接可寻址 |
| 引用块、预格式化、行内代码、代码块 | ✅/🟡 | pre/code 等宽字体；无语法高亮 |
| ul/ol/li | 🆕 | ul disc、ol decimal、`list-style-type:none`；无 `::marker`、CSS counter、reversed |
| dl/dt/dd | 🟡→✅（第 48 轮） | 普通块级堆叠；dd 补 UA 40px 缩进 |
| mark、abbr、分隔线、换行符 | ✅（第 48 轮） | mark 高亮背景；abbr 虚线下划线；del/ins/u 语义装饰线；hr 不再 0 高 |
| table/thead/tbody/tfoot/tr/td/th | 🆕 | 真实行列网格、自动列宽、受控 colspan/rowspan；见下节 |
| details/summary | 🆕→✅（第 48 轮） | 关闭只显示 summary，summary 点击切换；补 disclosure triangle；无动画 |
| dialog | 🟡 | 无 `open` 不渲染，有 `open` 显示；无 top-layer、::backdrop、焦点圈禁 |
| template | ✅ | 不渲染、不进可见快照 |
| slot | 🟡 | 当普通容器；无 Web Components 分发语义 |

### 3. 媒体与嵌入控件

| 控件 | 状态 | 说明/边界 |
| --- | --- | --- |
| 图片 | ✅ | HTTP/data 预取解码、固有尺寸、object-fit、截图 |
| 响应式图片 srcset/picture | ✅（第 45 轮） | 候选选择算法 + currentSrc 已落地 |
| 图片 alt | 🟡 | 可访问名用 alt；加载失败不画可见 alt 文本 |
| 画布 canvas | 🆕/✅ | HTML width/height/缺省 300×150 进布局；已有 RGBA 矩形/像素读写/PNG 回写 |
| Canvas 高级 2D | ⛔ | 路径、文字、渐变、远程图片入 Canvas、WebGL 未实现 |
| 矢量图形 svg | 🟡→✅（第 48 轮） | 固有几何 + MediaFrameNote 占位；仍不光栅化 path/text/gradient |
| 视频/音频/字幕轨道 | 🟡→✅（第 48 轮） | video/object/embed 替换式几何 + poster/data/src 预取 + 占位框；无解码/controls/播放状态；source/track 计入结构内容 |
| 内联框架 iframe | 🟡→✅（第 48 轮） | 缺省 300×150 + MediaFrameNote 占位；不加载/渲染子文档 |
| 嵌入对象/参数/object/embed | 🟡→✅（第 48 轮） | 替换式几何 + data/src 预取 + 占位框；不执行插件 |

### 4. 容器与布局控件

| 控件/能力 | 状态 | 说明/边界 |
| --- | --- | --- |
| div/section/article/aside/header/footer/main/nav/address/figure/figcaption | ✅/🟡 | 块级容器与文本流；无 UA 专属微语义 |
| 盒模型 | ✅/🟡 | 项目默认 border-box 语义；百分比/包含块已有近似，非完整 CSS 盒模型 |
| 正常流/Flex/Grid/浮动/定位 | ✅/🟡 | Flex/Grid/Float/absolute/fixed 已有；第 48 轮补 absolute 静态位置；sticky 仍只按 relative，inline-flex 内部退化 |
| 堆叠与溢出 | 🟡 | z-index/opacity/transform/overflow 有近似；无完整层叠上下文、滚动条、负 z-index 语义 |
| 表单控件 display:block | 🆕 | 保留替换式固有尺寸，不再退化成 0 高全宽块 |
| 文本装饰/语义高亮 | ✅（第 48 轮） | del/ins/u/abbr/mark 语义装饰 + CSS `text-decoration` 子集；address 斜体仍缺字形链 |

### 5. 脚本动态生成控件

| 能力 | 状态 | 说明/边界 |
| --- | --- | --- |
| JS 增删节点/改样式后重排 | ✅ | `apply_js_reply` DOM 变化后全量重排、图片补齐、推帧 |
| 表单运行时值/选中态同步 | 🆕 | `apply_widget_state` 后 inspect/JS/布局/截图一致 |
| 自定义下拉/分页/轮播/树/标签页/手风琴/菜单/气泡/通知/评分/开关/穿梭框等 | 🟡 | 由页面 JS + 通用 DOM/CSS 渲染；无组件级专用行为 |
| Shadow DOM / Web Components 样式隔离 | ⛔ | 无 Shadow DOM 样式树；custom element 仅按普通 DOM |
| CSS 动画/过渡帧同步 | 🟡 | JS 改 DOM 会重排；无 requestAnimationFrame 级动画时序/过渡补间 |

## 三、基础表格实现边界

`layout/table.rs` 采用 occupancy grid + 两遍测量：

1. 收集 `thead/tbody/tfoot` 下的 `tr`，再收集 `tr` 的直接 `td/th`。
2. 第一遍对单元格做 max-content 测量，列宽取最大值；跨列格把超出量分摊到占位列。
3. 列宽总和按可用宽缩放/扩张（最小列宽 24px）。
4. 第二遍按最终列宽测量单元格 Fragment，行高取单元格外高最大值；跨行格把超出量分摊到占位行。
5. 单元格内部递归复用 `layout_children`，边框/背景复用显示列表发射器。

**已支持**：行列对齐、自动列宽、受控 `colspan/rowspan`（非法值钳制；重叠 rowspan 退化为各自占行并告警）、单元格内块级/Flex/Grid 内容、td/th UA 边框与 padding。

**非目标**：`border-collapse`、`caption`、`col/colgroup`、百分比列宽、`table-layout:fixed`、嵌套表格列宽联合求解、完整 CSS 表格规范。

## 四、显示异常排查映射

| 现象 | 优先检查 |
| --- | --- |
| 输入后截图仍是旧值 | 是否走了 `act_input/act_clear` 后的 `relayout_and_push`；`Document::apply_widget_state` 是否覆盖 value |
| checkbox 看不到勾 | `form_values` 是否为 `"true"`；`WidgetGlyphKind::CheckMark` 是否进 display list |
| select 把所有 option 都画出来 | option 是否 `display:none`；select 固有宽是否走 `select_display_text` |
| details 正文常驻 | `open` 覆盖是否落解析树；`ComputedStyle` 的 details 抑制标志是否传播 |
| 表格单元格上下错位 | `layout/table.rs` 的 occupancy grid 与 row_h 分配；colspan/rowspan 是否非法 |
| 控件 display:block 后高度为 0 | 是否命中 `is_form_control_tag` 的替换式固有尺寸分支 |
| JS 改 DOM 后仍旧布局 | `apply_js_reply` 的 `dom_changed/state_changed` 判定与 `layout_rev` 同步 |

## 五、测试矩阵

| 层 | 文件 | 覆盖 |
| --- | --- | --- |
| 文档 | `src/agent/browser/document.rs::tests` | 语义属性序列化保真 |
| 布局 | `src/agent/browser/layout/form_widget_tests.rs` | 运行时值、checkbox/radio/select/textarea、details/dialog、Canvas、marker、progress/meter |
| 媒体/语义（第 48 轮） | `src/agent/browser/layout/media_embed_tests.rs` | video/object/embed 几何、poster 预取、MediaFrameNote、del/ins 装饰、hr 非 0 高、mark 高亮 |
| 表格 | `src/agent/browser/layout/table_tests.rs` | 行列对齐、colspan/rowspan、非法跨距、单元格块级内容 |
| 样式 | `src/agent/browser/style/state_tests.rs` | checked/disabled、details 抑制、option/datalist 隐藏 |
| 渲染 | `src/agent/browser/render/widget_tests.rs` | WidgetGlyph 真实像素 |
| 端到端 | `tests/web_form_state_roundtrip.rs` | input/click/select/summary 后 inspect、表格几何、截图落盘 |

验证命令：

```bash
cargo test --lib
cargo test --test web_form_state_roundtrip
cargo test --test web_use_engine
cargo test --test browser_canvas_screenshot
cargo test --no-fail-fast
./rebuild_restart_app.sh
```

## 六、后续路线

1. select 原生弹层/multiple、date/color/range 专用 UI。
2. ~~`srcset/picture` 候选选择与 `currentSrc`~~（第 45 轮已完成）。
3. `border-collapse`/caption/colgroup/百分比列宽的完整表格算法。
4. dialog top-layer/::backdrop/焦点圈禁。
5. SVG 子集光栅化；video poster 静态显示与 iframe 子文档隔离（poster/占位第 48 轮已落地，子文档隔离仍非目标）。
6. Shadow DOM 与动画帧同步；sticky 真吸附。
