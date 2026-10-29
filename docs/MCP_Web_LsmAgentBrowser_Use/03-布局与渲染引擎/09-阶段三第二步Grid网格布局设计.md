# 阶段三第二步:Grid 网格布局设计

> 本文是 `../01-总体设计与架构/07-全功能浏览器能力矩阵与后续阶段实施方案.md` 中 **Phase 3** 的第二轮落地设计。
> 本轮交付:**CSS Grid 显式/隐式网格 + 轨道尺寸(px/%/fr/auto/min-content/max-content/minmax/repeat)
> + 自动放置 + 显式放置(span/行列线号) + gap + 条目/轨道级对齐**。
> 图片渲染、原生窗口、overflow 裁剪在后续轮次按 08 号文档预留接口落地。
>
> 状态标记:✅ 本轮实现;🟡 部分实现/有边界(边界逐条列出);⏳ 未实现(后续轮次)。

## 一、目标与范围

Agent 场景的痛点(承 08 号文档):导航条、卡片墙、表单网格、仪表盘大量使用
`display:grid`。当前 `display:grid` 落入 `is_block()` 纯块级流,子项纵向堆叠,
列对齐/坐标全部失真——`rect_for`、坐标点击、截图在这些页面不可信。

本轮目标:

1. **样式引擎**:`ComputedStyle` 增加 Grid 容器/条目的**强类型字段**,
   轨道列表在声明期解析为 `TrackSize` 枚举,级联/继承/`getComputedStyle` 全程可见;
2. **布局引擎**:新增 `layout/grid.rs` Grid 格式化上下文,复用 08 号文档的
   `measure_content`/`Fragment` 局部度量原语;
3. **接线**:`layout_children` 在 flex 分支旁增加 `display:grid` 判路;
4. **契约不变**:`compute` 签名与 `Layout`/`ElementBox` 字段零改动,
   `MCP_Web_LsmAgentBrowser_Use` 兼容层零改动。

非目标(明确不做):`dense` 稀疏回填、命名网格线/区域(`[name]`/`grid-template-areas`)、
subgrid、masonry、`span` 负值、`repeat(auto-fill/auto-fit)`、fr 行在不定高容器下的
精确 max-content 分解、嵌套百分比行高、表格布局。

## 二、样式层设计

### 2.1 新类型(`style/values.rs`)

```rust
/// 轨道尺寸(声明期解析,布局期只读)。
pub enum TrackSize {
    Px(f64),
    Percent(f64),        // 列按内容宽;行按容器高(高度不定退化 auto)
    Fr(f64),
    Auto,
    MinContent,
    MaxContent,
    MinMax(Box<TrackSize>, Box<TrackSize>),  // 分量不再嵌套 minmax
}
/// 网格线(条目放置)。
pub enum GridLine { Auto, Int(i64), Span(u32) }
pub enum GridFlow { Row, Column }   // dense 后缀接受但忽略(🟡)

pub struct GridContainerStyle {
    pub template_cols: Vec<TrackSize>,
    pub template_rows: Vec<TrackSize>,
    pub auto_flow: GridFlow,
    pub auto_cols: TrackSize,       // 隐式列尺寸,默认 Auto
    pub auto_rows: TrackSize,       // 隐式行尺寸,默认 Auto
    pub justify_items: String,      // stretch | start | center | end
    pub align_items: String,        // 同上
    pub justify_content: String,    // 轨道整体分布(flex 同关键字域)
    pub align_content: String,
    pub row_gap: f64,
    pub col_gap: f64,
}
pub struct GridItemStyle {
    pub col_start: GridLine,
    pub col_end: GridLine,
    pub row_start: GridLine,
    pub row_end: GridLine,
    pub justify_self: String,       // auto | stretch | start | center | end
    pub align_self: String,
}
```

`ComputedStyle` 增加 `grid: GridContainerStyle`、`grid_item: GridItemStyle`。
均为非继承属性,`ua_default` 的 `..Default::default()` 自动逐元素重置。

### 2.2 声明解析(`apply_declaration` 扩展)

| 声明 | 行为 |
| --- | --- |
| `grid-template-columns/rows` | 轨道列表解析(下表);`none` → 空表 |
| `grid-auto-columns/rows` | 单个轨道尺寸 |
| `grid-auto-flow` | `row`/`column`(+`dense` 忽略);`row dense` 等同 `row` |
| `grid-column` / `grid-row` | 简写 `start / end`,`/` 省略时 end=start+1 |
| `grid-column-start/end`、`grid-row-start/end` | 整数 / `span N` / `auto` |
| `justify-items` / `justify-self` | 关键字白名单(`normal`→`stretch`) |
| `align-items` / `align-self` | **共享关键字**:同时写入 `flex`/`flex_item` 与 `grid`/`grid_item` |
| `justify-content` / `align-content` / `gap` / `row-gap` / `column-gap` | **共享**:同时写入 flex 与 grid 两侧(容器只读自己那侧,互不干扰) |
| `repeat(n, …)` | 仅整数次数展开(每项可为任意轨道尺寸);`auto-fill/auto-fit` ⏳ 丢弃 |
| `minmax(a, b)` | 两分量各自轨道尺寸(分量不再嵌套 minmax/`repeat`) |

**轨道列表词法**:顶层按空白/逗号切分(括号内递归),支持
`px`/`%`/`fr`/`auto`/`min-content`/`max-content`/`minmax()`/`repeat()`;
无法识别的词整体丢弃(宁缺勿错)。

`to_map()` 展平输出 `grid-template-columns/rows`(重新序列化文本)、
`grid-auto-flow`、`justify-items/self` 等,`getComputedStyle`/inspect 可见。

## 三、布局引擎设计(`layout/grid.rs`)

### 3.1 入口与容器盒

`grid::layout_container` 与 `flex::layout_container` 同构:外边距/边框/内边距按
块级惯例,背景盒先占位、结束后回填高度,内容区交给 Grid 算法,返回内容高度。
`layout_children` 在 `display:flex` 判路旁增加 `display:grid`。

### 3.2 条目收集与放置

- 收集与 Flex 相同:元素子节点 + 匿名文本段;`position:absolute/fixed` 延后;
  `display:none` 跳过。
- **显式放置**:任一轴给出 `Int/Span` 即参与。线号 1 基;负线号按显式轨道数
  从末尾折算(显式轨道不足时退化为 auto);`end = start + span`。
- **自动放置**(sparse,无 dense 回填):`auto-flow:row` 行优先游标扫描,
  找到第一个「span 矩形不与已占格冲突」的空位;`auto-flow:column` 列优先。
  游标单调前进,不回头(与浏览器 sparse 语义一致)。
- **隐式轨道**:放置结果超出显式模板时按需生成,尺寸取
  `grid-auto-columns/rows`;占用表 `Vec<Vec<bool>>` 随放置增长。

### 3.3 轨道尺寸分配

**列**(行对称;行高在列确定后按内容度量):

1. `Px` → 定值;`Percent` → 按内容宽(行:容器高定值时按容器高,否则按 auto);
2. `auto`/`min-content`/`max-content`/`minmax(min,max)` 的内禀部分:
   对**单跨度**落于该轨道的条目取 max-content 度量
   (`measure_content` 以 `MAX_CONTENT_WIDTH` 跑一遍取最右/最下界,
   复用 08 号文档原语);`min-content` 取最长单词宽(嵌套盒不递归,见边界);
   `minmax` 用 `[min, max]` 钳制内禀值;
3. `fr` 轨道:基数 = 内禀 min(取 0 或单跨度内容的 min-content,本轮取 0,🟡),
   再按 `fr` 权重分配剩余空间 `free = 容器 - Σ非 fr - Σgap`;
4. `free > 0` 且无 `fr`:均分给 `auto` 轨道(浏览器 `normal/stretch` 的近似);
5. `free < 0`:不收缩定值轨道(溢出保形,🟡)。

**行高**:列宽确定后,每行取「行内条目按其列宽测量的内容高 + padding/margin」
的最大值;显式 `height` 的条目以其显式高参与;`fr` 行在不定高容器下按内容高(🟡)。

### 3.4 条目摆放与输出

条目区域 = 所跨轨道之和 + 内部 gap。内容以
`measure_content(node, inner_w)` 测量;最终盒尺寸:

- 宽/高 = 显式 `width/height` ?? (`stretch` ? 区域尺寸 : 内容尺寸);
- `justify-self`/`align-self`/`justify-items`/`align-items` 在区域内定位
  (`start`/`center`/`end`/`stretch`,缺省 `stretch`);
- 输出与 Flex 一致:`ElementBox` + 内容片段平移注入;匿名文本按区域宽换行。

容器内容高 = Σ行高 + 行 gap;`align-content`/`justify-content` 用于轨道整体在
容器内的分布(内容高/宽小于容器时的余量分配,复用 flex 的 `justify_offsets` 语义)。

### 3.5 绘制序

沿用 08 号文档:`ElementBox.z_index`,`compute_with` 返回前稳定排序。Grid 不新增
层叠语义。

## 四、已知边界(🟡)

1. **dense 不支持**:`grid-auto-flow: * dense` 按 sparse 处理;
2. **fr 轨道内禀下限取 0**:内容极宽时 fr 列可能窄于内容(文字换行压行),
   浏览器行为是 min-content 下限;
3. **`repeat(auto-fill/auto-fit)` 丢弃**:轨道词法无法识别时整段丢弃;
4. **命名线/`grid-template-areas` 不解析**:关键字丢弃,放置退化自动流;
5. **% 行高在不定高容器退化 auto**;嵌套 % 不递归求解;
6. **负线号**按显式轨道数折算,隐式轨道不参与负数锚定;
7. **`minmax` 分量不嵌套** `minmax`/`fr` 复合;分量为 `fr` 时按 `auto` 处理;
8. **baseline 对齐缺失**(`align-items:baseline` 按 `start`);
9. **匿名文本段**各自独立成条目,跨行内元素合并不做(与 flex 边界一致);
10. **行 `fr` 在不定高容器按内容高**:不追求与浏览器一致的 max-content 分解;
11. **min-content 取最长单词宽**:嵌套块级子盒不递归分解;
12. **多跨度条目不参与轨道内禀生长**:`span N` 条目不撑大 auto 轨道,
    仅在区域内摆放(可能溢出);
13. **一轴定值、一轴 auto 的条目**在 auto 轴从 0 起取最小空位(近似 sparse,
    非浏览器的完整稀疏游标语义)。

## 五、验证

- 单测(`layout/tests.rs` 新增 Grid 段,15 个用例全绿):
  - px 轨道列宽;`fr` 按权重分配;`repeat(3,1fr)` 展开;`minmax` 钳制;
  - 自动流行优先换行 / `grid-auto-flow:column` 列优先;
  - `gap` 行列间隔;`span 2` 跨列;显式 `grid-column`/`grid-row` 放置;
  - 隐式行高由内容决定;`justify-items:center` 条目居中;
  - 无模板单列堆叠(浏览器一致);容器高度 = Σ行高 + gap;grid 嵌套 flex;
- 样式单测(`style/tests.rs`):轨道词法解析(`fr`/`minmax`/`repeat`/非法丢弃)、
  共享关键字双写、`getComputedStyle` 可见;
- 全量 `cargo test`(`macos_ax_smoke` 1 例 TCC 环境失败为存量,与本轮无关);
  `cargo build` 通过;契约不变(`compute` 签名与 `Layout`/`ElementBox` 字段零改动)。

## 六、后续轮次(承 08 号文档接口)

| 事项 | 落点 |
| --- | --- |
| 图片渲染 | `Layout` 增加图元列表,渲染器第二绘制 pass |
| overflow 裁剪 | `Layout` 携带裁剪矩形,渲染器按矩形 clip |
| 原生窗口/输入 | 布局坐标已是页坐标,窗口层只做视口映射 |
| 脏标记重排 | `measure_content` 原语即增量子树边界 |
| dense 回填 / 命名线 | `grid.rs` 放置器内扩展 |
