# 56 - Canvas 2D 路径/文字/渐变/图像源宿主光栅化方案

## 背景与缺口

对照 40 号文档的全量控件矩阵,Canvas 高级 2D 是显示/渲染系统最大的显式 ⛔ 缺口。
第 56 轮实施前的现状(`script/canvas_js.rs` 旧实现):

| API 组 | 旧状态 |
| --- | --- |
| `beginPath/moveTo/lineTo/arc/arcTo/ellipse/quadraticCurveTo/bezierCurveTo/rect/closePath/fill/stroke/clip` | 全部 `noop` |
| `translate/rotate/scale/transform/setTransform/resetTransform/save/restore` | 全部 `noop` |
| `fillText/strokeText` | `noop`;`measureText` 按 `字数×0.55×size` 估算 |
| `createLinearGradient/createRadialGradient/createPattern` | 返回哑对象,画不出渐变 |
| `drawImage` | 仅 Canvas→Canvas(最近邻) |
| `setLineDash/getLineDash/isPointInPath` | `noop`/空 |

真实影响面:**图表库**(ECharts/Chart.js,路径+文字+渐变)、**行为验证码**
(GeeTest 滑块,canvas 路径画缺口)、**地图瓦片 canvas 图层**(`drawImage(img)`)、
**指纹画布**(`fillText/arc/stroke` 取色)此前全部画不出来或拿到错误数据。

另一层结构性问题:旧实现在 **Boa 里逐像素循环**绘制(`fillRect` 双重循环 +
`__canvasBlend`),2000×200 指纹画布的 `toDataURL` 曾实测 >50s(京东 pc-tk
首屏卡死,当时靠 PNG 编码下沉缓解;绘制本身仍是 JS 逐像素)。路径级光栅化
若继续留在 Boa 将不可用。

## 方案:像素权威迁移宿主

画布像素/路径/变换/渐变的**权威状态**从 JS 堆迁移到 JS 线程内的 Rust 侧
(thread_local),JS 侧 `ctx` 只保留**薄壳**:所有绘制命令序列化成 JSON 指令,
经单一原生入口 `__lsmCv(key, opJson, extra?)` 分发到宿主光栅器。

```
页面脚本                     JS 线程(Boa)                      宿主(Rust)
ctx.fillRect(...)  ──JSON──▶ __lsmCv(key, op) ──▶ canvas_raster::handle
ctx.getImageData() ◀─Uint8Array──────────────────  CovBuf/像素
__lsmFlushCanvases ──to_data_url(按 rev 缓存)──▶   data-lsm-canvas 属性
                                                        │(通道零改动)
                                                        ▼
                                              images.rs 预取解码 → 布局渲染
```

`data-lsm-canvas` → 宿主图片管线通道**零改动**:`__lsmFlushCanvases` 仍把
PNG data URL 写进画布属性,`images.rs::collect_canvas_images` 照旧预取解码,
布局/渲染/截图不感知本轮变化。

### 复用既有光栅原语

| 能力 | 复用 |
| --- | --- |
| 填充(nonzero/evenodd) | 扫描线 2×2 超采样卷绕判定(与 `render/raster.rs::draw_svg_shape` 同思路) |
| 描边(AA、butt/round/square cap、round join) | 距离场(与 `stroke_coverage` 同思路)+ 虚线弧长分段 |
| 阴影(`shadowBlur/shadowColor/shadowOffset*`) | `render::blur_mask` 三趟盒式模糊(σ = blur/2) |
| 文字(`fillText/strokeText/measureText`) | `FontSet` 真实字体度量 + 字形光栅缓存(无字体回退占位色条) |
| 变换(`translate/rotate/scale/transform/…`) | `style::transform::Affine`(2×3 仿射,canvas 后乘语义) |
| PNG 编码 | `image` crate;编码结果按像素版本缓存(`pixel_rev`/`enc_rev`) |
| 图片源取回 | `data:` 本地解码;`http(s)` 走 SSRF/TLS 门禁 + `reqctx::Dest::Image` 指纹头阻塞 fetch(15s) |

## 实施清单(本轮代码)

| 落点 | 内容 |
| --- | --- |
| `src/agent/browser/script/canvas_raster.rs`(新增) | 宿主光栅器:状态机(像素/路径/CTM/裁剪/虚线/save栈)、30 op 分发、填充/描边/文字/渐变/贴图/阴影/复合、PNG 编码缓存 |
| `src/agent/browser/script/runtime.rs` | 注册 `__lsmCv` 原生(typed array 出入,`JsUint8Array::from_iter`);`fetch_image_bytes`(data:/http 双通道);`percent_decode` |
| `src/agent/browser/script/canvas_js.rs`(重写) | JS 薄壳:全部原 noop 方法变真;渐变对象 `addColorStop`;`measureText` 真度量;`drawImage` 三类源;flush 走宿主编码 |
| `src/agent/browser/render/mod.rs` | `pub(crate) use blur::blur_mask`(canvas_raster 复用三趟模糊) |
| `tests/browser_canvas_screenshot.rs` | 页面级端到端:圆环描边 + 水平渐变条 + 文字进入真实截图(三色采样) |

### 关键语义决策

1. **CTM 合成是后乘**:`st.ctm = st.ctm.compose(&next)`——`translate` 后
   `rotate` = 绕平移后的原点旋转(浏览器语义;写成先乘会把平移向量也转进去)。
2. **路径点在构建时即变换进设备空间**(对仿射成立,等价于绘制时变换);
   `cur` 游标保留**用户空间**供 `arcTo`/弧续接。
3. **`fillRect/strokeRect/clearRect` 四角各自过 CTM**:旋转/斜切下是变换后的
   平行四边形,不是「左上角变换 + 宽高轴对齐」。
4. **`fill` 把开放子路径按闭合处理**(卷绕计算隐式补尾→首段);`clip` 保留
   最近一次路径(多 clip 交集由覆盖率逐像素相乘近似)。
5. **`copy` 复合先清全画布再按 source-over 上色**;`destination-out`/`xor`/
   `lighter` 在直行(非预乘)RGBA 上逐像素合成。
6. **`drawImage` 逆映射**:目标矩形四角过 CTM 取 AABB,逐像素经 CTM 逆变换
   回源坐标采样(`imageSmoothingEnabled` 开 → 双线性,关 → 最近邻);
   源矩形越界部分裁掉。源支持:同线程其它画布 key / 已解码图片 id。
7. **图片源懒解码缓存**:img-like 元素首次作为 `drawImage` 源时取回解码
   (URL→id 缓存,LRU 24 张),并回填 `naturalWidth/Height`。
8. **`toDataURL`/flush 按像素版本缓存**:同一轮多次 flush 不重复编码
   (指纹画布高频 `toDataURL` 场景的关键优化)。
9. **`stroke_text` 用形态学膨胀近似**(字形蒙版 ±line_w/2 邻域取最大);
   字形本身不随 CTM 旋转/斜切(平移 + `sqrt(|det|)` 等比缩放近似)。
10. **尺寸边界**:单边 ≤4096(JS 侧同一上限)、像素总量 ≤64MB、图片源
    ≤`LAEW_WEB_MAX_IMAGE_BYTES`(默认 12MB,与宿主图片管线同源)。

## 已知边界(本轮非目标)

- `createPattern` 返回哑对象(赋值后不画);`Path2D` 构造器、`ctx.filter`、
  离屏 canvas 级联未实现。
- `miterLimit` 不生效(miter/bevel join 由距离场近似,凸角略有缺口);
  圆角 join 与圆 cap 由端点圆盘精确覆盖。
- radial 渐变按同心圆近似(非同心 focal 场景有偏差)。
- `drawImage` 不参与阴影(shadow* 对图像无效)。
- WebGL 维持 `capability_gaps` 如实上报(canvas 2D 之外的空白仍有第 50 轮
  的 `MediaFrameNote` 占位与提示)。
- canvas 内部像素 = CSS 像素 ×1(无 DPR 缩放;与既有 `<canvas>` 布局语义一致)。

## 测试矩阵

| 层 | 文件 | 用例 |
| --- | --- | --- |
| 光栅器单测 | `canvas_raster.rs::tests`(15 例) | 矩形/三角形/圆弧 AA 填充、平移、clip、线性/径向渐变取色、虚线间隔、destination-out、save/restore、toDataURL rev 缓存、画布互贴、measure、clearRect、颜色解析 |
| JS 线程端到端 | `script/tests.rs`(7 例) | 圆弧填充取色、渐变+虚线、文字度量与落墨、变换/裁剪/save-restore、画布互贴与 put/getImageData、路径画布 toDataURL、既有大画布性能回归 |
| 页面级端到端 | `tests/browser_canvas_screenshot.rs` | 真实页面脚本画「渐变条 + 圆环描边 + 文字」→ 截图落盘 → 三点像素采样 |

验证命令:

```bash
cargo test --lib                                  # 全量单测
cargo test --lib canvas_raster                     # 光栅器
cargo test --lib "script::tests::canvas"           # JS 线程端到端
cargo test --test browser_canvas_screenshot        # 页面级
cargo test --test web_form_state_roundtrip         # 像素通道回归
```

## 站点收益预期

- **ECharts/Chart.js 图表**:折线/柱状/饼图(canvas 路径+渐变+文字)从空白变为可渲染;
- **GeeTest 行为验证码**:拼图缺口与干扰块(canvas 路径)可见,配合既有自愈循环;
- **地图瓦片 canvas 图层**:`drawImage(img)` 可贴真实瓦片(配合既有降级栅格路径);
- **指纹画布**:绘制与编码全链原生,`toDataURL` 按版本缓存,首屏卡死风险消除。
