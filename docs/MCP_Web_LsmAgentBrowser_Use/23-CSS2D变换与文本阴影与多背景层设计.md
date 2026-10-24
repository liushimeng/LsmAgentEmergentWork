# 23 - CSS 2D transform + text-shadow + 多背景层设计与实现

> 阶段六第二步。承接 `22-专业渲染引擎重构设计与实现.md` 第六节路线图第 4 项
> （「transform 矩阵逐项仿射」）与高频视觉能力缺口。全部落在既有
> `tiny-skia + fontdue + 自研代码` 上，**零新增 crate、零系统依赖、离线可编译**，
> macOS / Windows / Linux 渲染结果逐像素一致。

## 一、背景与问题

22 号文档落地显示列表架构后，渲染层仍有三类高频视觉能力缺失（真实页面截图
保真度的直接缺口）：

1. **CSS 2D transform**：`values.rs` 把 `transform` 归入 `extra` 直接丢弃，
   旋转/缩放/平移全部退化为恒等。现代 Web UI（卡片、按钮、图标、悬浮层、
   动画初始帧）大量使用，缺失后截图与真实浏览器差异显著。
2. **text-shadow**：解析期丢弃，文本阴影（hero 标题、按钮文字）整块丢失。
3. **多背景层**：`background-image` 只取首层（`parse_first_gradient` 注释
   明确「多背景只取首层」），`url(a), linear-gradient(...)` 混排时第二层起
   全部丢失。

## 二、参考技术方案的映射与取舍

| 参考方案组件 | 本工程决策 | 理由 |
| --- | --- | --- |
| WebRender 显示列表 + GPU 合成 | 已在用（22 号文档）；本轮把「逐项仿射」补上 | 显示列表项自带 `(z,seq,rank)` 与裁剪/透明度，天然可挂仿射矩阵 |
| 脏标记增量布局/渲染 | 仍列路线图 | 本轮先补齐正确性；增量优化按实测热点再引入 |
| 多线程并行样式/布局/分块渲染 | 仍列路线图 | 同上 |
| stylo / rusty_v8 / wgpu | 不引入 | 同 22 号文档（离线可编译铁律 / 本机 GitHub 不可达） |

## 三、总体架构

```
样式(style/)                    布局(layout/)                     渲染(render/)
┌──────────────────────┐   ┌────────────────────────────┐   ┌────────────────────────┐
│ Transform2D(操作序列) │──▶│ apply_group 回补:子树前乘   │──▶│ Mapper(逆映射)          │
│ TransformOrigin       │   │ emit_*:自身变换            │   │  画布像素 → 局部(页)坐标│
│ TextShadow[]          │   │ InlineRun 携带行内变换      │   │ 各 draw_* 局部坐标判定  │
│ BgLayer[]             │   │ background_layers 逆序发射 │   │ TextShadow 蒙版+模糊    │
└──────────────────────┘   └────────────────────────────┘   └────────────────────────┘
```

## 四、CSS 2D transform

### 4.1 样式层（`style/transform.rs`，新增模块）

`transform` 的百分比平移相对元素**自身盒尺寸**，样式层无从得知，故解析为
**操作序列** `Transform2D { ops: Vec<TransformOp> }`，布局期按盒尺寸求解为 `Affine`：

```rust
pub enum TransformOp {
    Translate(Length, Length), TranslateX(Length), TranslateY(Length),
    Rotate(f64),                     // 度
    Scale(f64, f64), ScaleX(f64), ScaleY(f64),
    Skew(f64, f64), SkewX(f64), SkewY(f64),
    Matrix(f64, f64, f64, f64, Length, Length),
}
pub struct Affine { a, b, c, d, e, f }   // x' = a·x + c·y + e ; y' = b·x + d·y + f
```

- 解析：`translate/translateX/translateY`（长度，`%` 相对盒宽/高）、
  `rotate`（`deg/rad/grad/turn`）、`scale/scaleX/scaleY`、`skew/skewX/skewY`、
  `matrix(...)`。多函数按声明序**左乘**（`M = Mn ∘ … ∘ M2 ∘ M1`），
  与 CSS「依次施加」语义一致。
- `transform-origin`：关键字 `left/center/right/top/bottom` + 长度/百分比，
  存 `TransformOrigin { x: Length, y: Length }`（默认 `50% 50%`），布局期求解。

**含原点变换的推导**（`resolve_with_origin`）：

```
T(p) = origin + M·(p − origin)
     = A·p + (origin − A·origin + t)      （M = (A | t)，t 为 M 自身平移）
⇒ T = { A,  origin − A·origin + t }
```

> 易错点：`M·(p−origin) ≠ M·p − M·origin`（M 含平移时两者差一个 `t`）。
> 早期实现漏掉 `+t`，导致 `translate(...)` 绕原点后退化为恒等 —— 已由单测
> `含原点平移保留平移量` 锁定。

### 4.2 显示项携带变换（`render/display.rs`）

```rust
pub struct DisplayItem {
    ...
    pub transform: Option<Affine>,   // 局部(页坐标) → 页坐标
}
impl DisplayItem {
    /// 前乘合成祖先变换：self.transform = outer ∘ self.transform
    pub fn compose_transform(&mut self, outer: Affine);
}
```

### 4.3 布局期合成（`layout/mod.rs`）

**元素盒高在子代布局完成后才定**，故不能用「进入压栈 / 退出弹栈」的变换栈
（子代布局时盒高未知），改用与 `opacity`/`overflow` 同款的**回补**：

```
mark = display.len()            // 子树发射前
<递归布局子树>                   // 子树项不带本元素变换
apply_group(mark, style, 盒矩形):
    if style.transform → 对 display[mark..] 前乘 T_E      // 子树随元素变换
emit_element(自身视觉项, transform = T_E)                  // 自身取自身变换
```

- **祖先链合成天然成立**：父元素 `apply_group` 在子元素 `emit_element` **之后**
  运行，故子元素自身项（已带 `T_child`）被前乘 `T_parent` → `T_parent ∘ T_child`。
- **自身背景/边框不受自身 overflow 裁剪**（CSS 语义）—— 自身项在 `apply_group`
  之后发射，天然免于自身裁剪/二次透明；变换同理（自身项带 `T_E`，正确）。
- **相对定位偏移**：`relative/sticky` 偏移改为**先平移子树几何、再以偏移后盒矩形
  做回补与自身发射**，使 `transform-origin` 与偏移一致。
- **行内元素**：行内文本在父级 `flush_inline` 时才发射（变换栈里没有它），故
  行内元素把**已求解的变换**随 `InlineRun` 带到 `flush` 时应用；嵌套行内元素
  用 `inherited_tf` 参数逐级合成（`T_e ∘ T_ancestor`）。

### 4.4 光栅化：逆映射（`render/raster.rs`）

新增 `Mapper`：画布像素 → 项局部（页坐标）。

```rust
struct Mapper { inv: Option<Affine>, scroll_y: f64, item: &DisplayItem }
impl Mapper {
    fn local(&self, px, py) -> (f64, f64)        // 无变换: (px+.5, py+.5+scroll_y)
    fn canvas_aabb(&self, x, y, w, h) -> (..)    // 局部盒变换后的画布 AABB
    fn scissor_of(&self, sc, x, y, w, h) -> Scissor
}
```

全部图元改为**局部坐标判定 + 画布像素迭代**：

| 图元 | 改造 |
| --- | --- |
| 背景（纯色/渐变） | `inside_rounded` / `gradient_t` 用局部坐标 |
| 边框环带 | 内缩矩形与环带判定用局部坐标 |
| 阴影 | 掩模建在**画布坐标**（模糊是屏上操作），掩模像素先 `mapper.local` 反算局部坐标再判 `inside_rounded`；`blur=0` 走实心圆角矩形路径 |
| 图片 | `u/v` 用局部坐标反算采样（`object-fit` 与 cover/none 语义不变） |
| 文本 | 逐字形：字形局部矩形 → 变换后画布 AABB → 逐像素反算局部坐标取 coverage |
| canvas 占位 | 同背景路径 |

**无变换路径逐像素等价**：`local = (px+0.5, py+0.5+scroll_y)`，形状判定用
`(item.x, item.y)`，与重构前「画布坐标 == 页坐标 − scroll_y」一致 —— 既有
渲染单测（含手构 `Layout`、legacy 回退）零改动即回归锚点。

## 五、text-shadow

- **样式层**：`ComputedStyle.text_shadow: Vec<TextShadow>`（`dx/dy/blur/color`，
  `currentColor` 留到布局期代入文字色；`transparent` 层丢弃）。
- **布局层**：`emit_text` 为**每层阴影**发射一个 `DisplayKind::TextShadow`
  （几何同文本行，`x/y` 加偏移，颜色 alpha 折进项透明度，`rank=5` 排在
  `Text`(`rank=6`) 之前）。块级文本的阴影经 `InlineRun` 携带；行内文本的阴影
  随 `InlineRun` 的 `text_shadow` 携带。
- **光栅化**：`shadow_blur = Some(b)` 且 `b > 0.5` 时走
  「字形覆盖蒙版 → 三趟盒式模糊（复用 `blur.rs`）→ 着色混合」；否则逐字形
  直接混合（与正文同路径）。蒙版像素总量受 `MAX_MASK_PIXELS` 约束。

## 六、多背景层

- **样式层**：`background_image: Option<String>` / `background_gradient` 统一为
  `background_layers: Vec<BgLayer>`，`enum BgLayer { Image(String), Gradient(Arc<GradientSpec>) }`。
  `background-image` 与 `background` 简写均按**顶层逗号**切层、逐层识别
  `url()`/`gradient()`，**保持声明序**（括号感知切分，渐变参数内逗号不拆层）。
  `to_map` 输出层原文（逗号连接），保持 `getComputedStyle`/`inspect` 可观测。
- **布局层**：`emit_element` 逆序发射背景层（**CSS 首层在最上** → 先画者被覆盖，
  故声明序最后一层先画、首层最后画）；图片层经 `bg_paint_for_url` 解析（复用
  既有 `background-position`/`background-size`/精灵图语义）。
- **光栅化**：无需改动（逐项绘制已支持）；叠画次序由发射序保证。

## 七、文件落点

```
src/agent/browser/
  style/transform.rs      新增：Affine + transform / transform-origin 解析
  style/values.rs         ComputedStyle 增 transform/transform_origin/text_shadow/
                           background_layers；解析分派；apply_initial 复位；to_map
  style/mod.rs            pub use transform::{Affine, Length, Transform2D, TransformOrigin}
  style/tests.rs          transform / transform-origin / text-shadow / 多背景层解析单测
  layout/mod.rs           own_transform；apply_group 前乘变换；emit_* 写变换；
                          relative 偏移先平移后回补；行内 InlineRun 携带变换；
                          background_layers 逆序发射；push_styled 签名扩展
  layout/flex.rs / grid.rs finish_container 签名调整；条目 emit_image 带变换
  render/display.rs       DisplayItem.transform + compose_transform；DisplayKind::TextShadow
  render/raster.rs        Mapper（逆映射）+ 各 draw_* 局部坐标化 + TextShadow 蒙版模糊
  render/tests.rs         transform / text-shadow / 多背景层像素断言
tests/web_use_engine.rs   富视觉页增三类元素 + 像素断言
```

单文件 ≤1800 行规约：transform 解析放**新模块** `style/transform.rs`（`values.rs`
存量已超限，不再膨胀）；`raster.rs` 由 796 增至约 1050 行（仍在限内）。

## 八、测试矩阵

| 层 | 用例 |
| --- | --- |
| style | `transform`：平移/百分比平移/`rotate(deg)`/多函数合成序/`transform-origin` 关键字与长度/含原点平移保留平移量/绕角旋转/逆变换还原/`none`/非法丢弃；`text-shadow`：多层/`currentColor`/blur；多背景层：`url`+`gradient` 混排保序 |
| layout | 变换回补：嵌套平移合成；相对偏移后原点正确 |
| render | 平移整体偏移；`rotate(45deg)` 角外为白；`scale(2)` 尺寸放大；`transform-origin` 绕右下角；嵌套祖先链合成；text-shadow 阴影色可见；模糊阴影红色像素；多背景层首层在上（红/蓝两向） |
| 集成 | `web_use_engine` 富视觉页 transform / text-shadow / 多背景层像素断言 |

**回归锚点**：既有渲染单测（含手构 `Layout`、legacy 回退、`cargo test --lib
browser::` 402 例）与 `web_use_engine` 19 例全绿。

## 九、边界与路线图

**本轮明确不做**（记录原因，防止误判为缺陷）：

- `transform` 的**布局效应**：transform 不改变占位几何、不建立 `fixed` 包含块
  （本内核布局层与显示层分离，本轮只影响「画」）；占位/命中几何变换列路线图。
- `translate3d/rotate3d/perspective` 等 3D、`transform-box`、`transform-style`。
- `text-shadow` 的多层离屏合成（逐层独立绘制，层间不混合）。
- `background-clip/origin/repeat/attachment` 语义细化（`repeat` 按不平铺近似）。
- 增量/脏矩形光栅化、GPU 后端：仍为路线图（22 号文档第六节）。

**路线图**（按收益排序）：命中几何随 transform 变换（点击/坐标定位精度）→
`transform` 的布局效应（含 fixed 包含块）→ 增量/脏区域局部重光栅 → GPU 后端
feature 门控 → 文本整形与字体栈。

## 十、实施与验收

1. 顺序：`style/transform.rs` → `values.rs` → `render/display.rs` + `render/raster.rs`
   → `layout/` 各 emit 点 → 单测 → 集成测试 → `./rebuild_restart_app.sh`。
2. 验收：`cargo build` + `cargo test --lib browser::` + `cargo test --test
   web_use_engine` 全绿。
3. 测试结束终止全部 laew 相关进程（含 `__browser-view` 查看器），释放资源。
