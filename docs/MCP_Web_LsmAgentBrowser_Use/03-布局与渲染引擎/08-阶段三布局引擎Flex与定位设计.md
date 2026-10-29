# 阶段三第一步:Flex 布局与定位体系设计

> 本文是 `../01-总体设计与架构/07-全功能浏览器能力矩阵与后续阶段实施方案.md` 中 **Phase 3** 的第一轮落地设计。
> 本轮交付:**Flex 弹性布局 + relative/absolute/fixed/sticky 定位 + z-index 绘制序**;
> Grid、图片渲染、原生窗口、滚动裁剪在后续轮次按本文预留的接口落地。
>
> 状态标记:✅ 本轮实现;🟡 部分实现/有边界(边界逐条列出);⏳ 未实现(后续轮次)。

## 一、目标与范围

Agent 场景的痛点排序(见 07 号文档):布局可信度决定截图、坐标点击、`rect_for`
定位是否可用。真实站点大量使用 `display:flex`(导航条、卡片、表单行)与
`position:absolute/fixed`(弹层、顶栏、遮罩),当前纯块级布局在这些页面上
几何失真,后续截图与坐标交互全部不可信。

本轮目标:

1. **样式引擎**:`ComputedStyle` 增加 Flex 容器/条目、定位、z-index 的**强类型字段**,
   级联/继承/`getComputedStyle` 全程可见;
2. **布局引擎**:块级流保持不变,新增 Flex 格式化上下文与定位体系;
3. **渲染**:按 z-index 稳定排序绘制盒,坐标类 API(`rect_for`/点击/滚动)自动受益;
4. **拆分**:`layout.rs` 转 `layout/` 目录(`mod.rs` + `flex.rs` + `position.rs` +
   `tests.rs`),为 Grid/图片留好扩展位。

非目标(明确不做):Grid、float、表格布局、inline-block 行盒、图片渲染、
溢出裁剪(overflow clip)、BIDI、动画/过渡、原生窗口。

## 二、样式层设计

### 2.1 ComputedStyle 扩展

新增三个内聚子结构(均为**非继承**属性,`ua_default` 每次重置,符合 CSS 规范):

```rust
pub struct PositionStyle {
    pub position: String,          // static | relative | absolute | fixed | sticky
    pub top/right/bottom/left: Option<f64>,   // auto → None
    pub z_index: i64,              // auto → 0
}
pub struct FlexContainerStyle {
    pub direction: FlexDirection,  // Row | RowReverse | Column | ColumnReverse
    pub wrap: FlexWrap,            // Nowrap | Wrap | WrapReverse
    pub justify: String,           // flex-start | flex-end | center | space-between | space-around | space-evenly
    pub align_items: String,       // stretch | flex-start | flex-end | center | baseline
    pub align_content: String,     // 同 justify 取值域 + stretch
    pub row_gap: f64,
    pub col_gap: f64,
}
pub struct FlexItemStyle {
    pub grow: f64,
    pub shrink: f64,
    pub basis: Option<f64>,        // auto/content → None
    pub align_self: String,        // auto | stretch | flex-start | flex-end | center
    pub order: i64,
}
```

`ComputedStyle` 增加 `pos: PositionStyle`、`flex: FlexContainerStyle`、
`flex_item: FlexItemStyle` 三个字段;`to_map()` 展平输出
(`position`/`top`/`z-index`/`flex-direction`/`justify-content`/`gap`/`flex-grow` 等),
`getComputedStyle` 与 `inspect(stylesheets)` 自动可见。

### 2.2 声明解析(`apply_declaration` 扩展)

| 声明 | 行为 |
| --- | --- |
| `position` | 关键字白名单,未知值丢弃 |
| `top/right/bottom/left` | `parse_length`,`auto` → `None` |
| `z-index` | 整数;`auto` → 0 |
| `flex-direction` / `flex-wrap` | 枚举映射 |
| `justify-content` / `align-items` / `align-content` / `align-self` | 关键字白名单(`start/end` 归一到 `flex-start/flex-end`) |
| `gap` / `row-gap` / `column-gap` | 1~2 个长度分量展开 |
| `flex-grow` / `flex-shrink` / `order` | 数值解析 |
| `flex-basis` | 长度;`auto`/`content`/`fill` → `None` |
| `flex` | 简写:`none`→`0 0 auto`;`auto`→`1 1 auto`;单数字→`g 1 auto`;`g s`/`g s b` 组合 |

百分比长度沿用 06 号文档既有简化(按视口宽估算),`flex-basis:50%` 同规则。

## 三、布局引擎设计

### 3.1 目录与职责

```
layout/
  mod.rs        LayoutCtx / 块级流 / 行内收集 / 文本换行 / Fragment 度量与平移
  flex.rs       Flex 格式化上下文(本轮核心)
  position.rs   定位体系:relative 偏移 / absolute/fixed 延后布置 / z-index
  tests.rs      全部布局单测(块级存量 + Flex + 定位)
```

`mod.rs` 承载结构体定义、入口 `compute`/`compute_with`、`LayoutCtx` 及既有块级/
行内逻辑(从原 `layout.rs` 逐行机械搬入,行为零改写)。

### 3.2 Fragment 度量原语(支撑 Flex/定位的关键重构)

既有引擎是"追加式"的:`self.y` 全局游标 + 直写 `self.lines/self.boxes`。
Flex 需要先知道条目内容尺寸再决定摆放,因此新增局部度量:

```rust
pub(crate) struct Fragment {
    pub height: f64,
    pub lines: Vec<TextLine>,
    pub boxes: Vec<ElementBox>,
}

impl LayoutCtx {
    /// 局部坐标(原点 0,0)下测量节点内容,返回片段;不产生任何全局副作用。
    pub(crate) fn measure_content(&mut self, node, content_width, style, depth) -> Fragment;
}
pub(crate) fn translate(fragment: &mut Fragment, dx: f64, dy: f64);
pub(crate) fn append(ctx: &mut LayoutCtx, fragment: Fragment);
```

实现:暂存 `y/lines/boxes` 长度,`y=0` 复用 `layout_children` 跑一遍,
`split_off` 截取增量后还原。max-content 度量 = 以超大宽度(无换行)测量。

### 3.3 块级流接线(最小侵入)

`layout_children` 在 `is_block` 分支之前插入三条判路:

1. `display:flex` → `flex::layout_container(...)`(容器自身盒、边距、padding 仍按
   块级惯例处理,内容区交给 flex);
2. `position:absolute/fixed` → 不进入流,记入**延后队列**;
3. `position:relative/sticky` → 按块级正常布局,产出后整体平移 `(left, top)`
   (sticky 无滚动语境,按 relative 处理,见 4.4)。

子树递归时**携带包含块** `cb: (x, y, w)`:进入定位元素时更新为其 padding 盒;
`fixed` 永远以视口 `(0, 0, viewport_w)` 为包含块。子节点全部布置完后统一处理
延后队列(`position::layout_abs`)。

### 3.4 Flex 算法(行方向为主,列方向对称)

**条目收集**:元素子节点(跳过 `display:none`);直接文本节点聚合成匿名条目
(每个连续文本段一个,白空格忽略)。条目内的行内/块级子树复用块级流布局。

**主尺寸解析**(以 row 为例,column 对调):

1. `base = flex-basis ?? width ?? max-content`(max-content = 超大宽度度量);
2. 行打包:`wrap` 时按 `Σ(outer main) + gap > inner main` 断行
   (outer = margin + border-box);`wrap-reverse` 行序反转;
3. 每行弹性分配:`free = inner_main - Σouter`:
   - `free > 0` 且 `Σgrow > 0`:按 `grow` 加权放大;
   - `free < 0` 且 `Σshrink > 0`:按 `shrink × base` 加权缩小;
   - 主轴尺寸随后按最终 inner main **重排内容**(文本换行高度随之修正);
4. `justify-content`:行内主轴定位(flex-start/end、center、
   space-between/around/evenly 均分剩余空间)。

**交叉轴**:`align-items`/`align-self` 决定条目在行内的交叉定位;
`stretch`(默认)时无显式交叉尺寸的条目拉伸到行交叉尺寸;`baseline` 暂按
`flex-start` 处理(字体度量不含基线表,🟡)。

**容器交叉尺寸**:nowrap = 最大行交叉;wrap = 行交叉和 + gap;
`align-content` 在容器交叉尺寸确定时分配行间剩余空间。

**输出**:每个条目一个 `ElementBox`(背景/可见性/z-index),内容片段平移注入;
容器盒由调用方按块级惯例产出。

### 3.5 绘制序

`ElementBox` 增加 `z_index: i64`;`compute_with` 返回前对 `boxes` 做
**稳定排序**(`sort_by_key(z_index)`)。渲染器既有"先盒后字"两层绘制保持不变;
负 z 盒与文字的层叠关系是已知边界(见 4.4),坐标 API 不受影响。

## 四、已知边界(🟡)

1. **sticky** 按 relative 处理:布局不感知滚动位置,粘滞效果不模拟;
2. **fixed** 在页面坐标系原点定位:渲染按页坐标减滚动偏移,固定元素会随页
   面"滚动",与真实浏览器视觉位置有偏差(坐标/截图语义仍自洽);
3. **baseline 对齐** 按 flex-start:字体栈不暴露基线偏移表;
4. **column wrap** 不支持:列方向断行罕见,遇到按 nowrap 处理;
5. **匿名盒合并** 简化:直接文本段各自成项,跨行内元素的文本合并不做;
6. **负 z 文字层叠**:文字统一绘制在盒层之上,负 z 容器内的文字不会被其
   背景遮挡;
7. **min/max-width** 仅 max-width 参与约束,min-width 未建模;
8. absolute 静态位置(无 left/top/right/bottom 时)按包含块原点摆放,
   不回溯流内位置;
9. **absolute `bottom` 锚定退化**:布局期父盒高度未定时(绝大多数场景),
   `bottom` 按 `top` 缺失处理,贴包含块上缘。

## 五、验证

- 单测(`layout/tests.rs`):
  - 存量块级用例原样搬入,保证零回归;
  - flex:横排 grow 均分、`justify-content:center` 居中、`gap`、wrap 两行、
    交叉轴 stretch、列方向、条目固定宽度、容器背景盒;
  - 定位:relative 偏移、absolute 尺寸收缩适配、fixed 视口包含块、
    z-index 排序、absolute 不占位;
- 全量 `cargo test`;契约不变(`compute` 签名与 `Layout`/`ElementBox` 字段
  只增不改),`MCP_Web_LsmAgentBrowser_Use` 兼容层零改动。

## 六、后续轮次(本文预留接口)

| 事项 | 落点 |
| --- | --- |
| Grid | `layout/grid.rs`,复用 `measure_content`/`Fragment` |
| 图片渲染 | `Layout` 增加图元列表,渲染器第二绘制 pass |
| overflow 裁剪 | `Layout` 携带裁剪矩形,渲染器按矩形 clip |
| 原生窗口/输入 | 布局坐标已是页坐标,窗口层只做视口映射 |
| 脏标记重排 | 当前全量重算,`measure_content` 原语就是未来增量子树的边界 |
