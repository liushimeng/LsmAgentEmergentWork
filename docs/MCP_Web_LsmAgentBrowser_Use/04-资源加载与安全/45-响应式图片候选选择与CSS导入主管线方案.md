# 45-响应式图片候选选择与 CSS 导入主管线方案

## 一、背景与根因

第 44 轮把**行内排版**与**控件视觉**补齐后,复核《Web 端控件全量清单与渲染设计要点》
第三节「媒体与嵌入控件」时发现两条**资源侧**断链 —— 二者的症状都是「页面少一块」,
但根因在资源主管线,与布局无关:

| # | 症状 | 实测(第 45 轮探针) | 根因 |
| --- | --- | --- | --- |
| 1 | 响应式图片整块空白 | 只给 `srcset`、不给 `src` 的 `<img>` 在页面上没有任何像素 | `images.rs` 只收集 `img[src]`;`srcset` 候选**既不预取也不排版** |
| 2 | `<picture>` 的候选不生效 | `<picture><source media=… srcset=…>` 永远走 `<img src>` | 无候选选择算法,`source` 元素完全不可见 |
| 3 | 站点样式整体丢失 | `@import url(base.css)` 的依赖表从不抓取,只有主表被内联 | `resource/import.rs::resolve_imports` **已实现但从未接线**(全仓库只有 re-export,零调用) |

第 3 条影响面最大:现代站点普遍把公共样式(变量/重置/组件)放在被导入的表里,
内核只取主表 → 页面**整页无样式**(白底黑字),此前一直被当作「站点自己没样式」
或「渲染层 bug」,实际是资源主管线少了一跳。

本轮把这两条资源链补齐。

---

## 二、本轮实施项总览

| 编号 | 主题 | 落点 | 状态 |
| --- | --- | --- | --- |
| F1 | `srcset`/`picture` 候选解析与选择 | `document.rs::parse_srcset/select_srcset/picture_srcset` | 🆕 |
| F2 | 图片管线按候选预取,布局按候选排版 | `images.rs::collect_image_urls` + `layout/mod.rs::image_src` | 🆕 |
| F3 | JS 桥 `img.currentSrc` | `script/prelude.rs` | 🆕 |
| F4 | CSS `@import` 接入主管线 | `resources.rs::inline_css_imports` + `resource/import.rs::import_urls` | 🆕 |

### F1 候选解析与选择

新增三个纯函数(`src/agent/browser/document.rs`):

| 函数 | 职责 |
| --- | --- |
| `parse_srcset(raw)` | 解析 `srcset` → `(URL, 描述符)` 列表;描述符为 `w`(宽度)/`x`(密度),无描述符按 `1x`。切分按 HTML 标准口径:**URL 取到空白为止、描述符取到逗号为止**(故 `data:` URL 里的逗号不会被误切) |
| `select_srcset(cands, viewport_width, dpr)` | 含 `w` 描述符:取「不小于源尺寸(视口宽 × DPR)的最小宽度候选」,没有则取最大;只含 `x`:取「不小于 DPR 的最小密度候选」,没有则取最大 |
| `picture_srcset(node, viewport_width)` | `<picture>` 内第一个命中的 `<source srcset>`;`media` 支持 `all`/`min-width`/`max-width`(px),`type` 只放行 `image/*`;未支持的条件一律**不匹配**(保守:不因未知条件选中错误候选) |

### F2 预取与排版

- **预取**(`images.rs::collect_image_urls`):每个 `<img>` 额外收集
  「`srcset`/`picture` 选中的候选」与「`src` 兜底」,两者都进图片管线。
  → 只给 `srcset` 的图片不再空白。
- **排版**(`layout/mod.rs`):`image_src` 改为「`srcset` 候选优先、`src` 兜底」;
  `natural_size` 在候选未加载成功时回落 `src` 的固有尺寸(盒高不至于塌成 0);
  `image_display_src` 决定**实际绘制**哪张(优先已加载的候选)。

**边界**:DPR 恒为 1(内核尚无 DPR 缩放),故 `x` 描述符一律选 1x 档;
`w` 描述符按**视口宽**近似源尺寸(浏览器按元素实际渲染宽度)。

### F3 `img.currentSrc`

站点常用 `img.currentSrc` 判断当前用的是哪一档(2x 图 vs 1x 图)。
JS 桥按密度描述符在 `__attrs.srcset` 上就地选出并返回;无 `srcset` 时回落 `src`。

### F4 CSS `@import` 主管线

`resources.rs::load_stylesheets` 在抓完主样式表后追加一跳:

```
1) 广度优先收集:每张主表的 @import → 解析绝对地址 → 入队(循环引用跳过)
2) 逐层抓取:强缓存优先 → 网络取回(带 Referer + Sec-Fetch-*(style) 指纹头)
   → 被导入表里的 @import 继续入队(深度上限 8 层,沿用 import.rs 口径)
3) 同步内联:import::resolve_imports 把 @import 原位换成被导入的 CSS 文本
   (带 media 条件的包成 @media 块)
```

**为什么抓取与内联分两段**:异步递归里 loader 闭包要同时借用 `Inner`(缓存)与
`HttpClient`,递归调用会撞借用检查。改成「先广度抓齐、再同步内联」后,
内联阶段只需查一张已抓好的表,`resolve_imports` 的同步签名可直接复用(该函数
此前是死代码,本轮首次真正被调用)。

**失败降级**:被导入表抓不到 → 该 `@import` 内联为空(主表其余规则照常生效),
并在 console 记一条 warn;网络台账照常记录。

---

## 三、控件矩阵更新(媒体与资源部分)

| 能力 | 状态 | 说明/边界 |
| --- | --- | --- |
| img `src` | ✅ | 预取解码、固有尺寸、object-fit、截图 |
| img `srcset`(密度 `x`) | ✅🆕 | 候选解析 + 选择 + 预取 + 排版 |
| img `srcset`(宽度 `w`) | ✅🆕 | 按视口宽近似选档 |
| `<picture>` + `<source>` | ✅🆕 | `media`(min/max-width)/`type`(image/*)过滤后取首个候选 |
| `img.currentSrc` | ✅🆕 | JS 桥按密度描述符就地选出 |
| `srcset` 候选加载失败回落 `src` | ✅🆕 | 显示回落 + 固有尺寸回落 |
| DPR 参与的候选选择 | ⛔ | DPR 恒为 1,2x 图不会因设备像素比被选中 |
| `sizes` 属性 | ⛔ | 属性保真,未参与 `w` 描述符的源尺寸计算 |
| 图片 alt 失败占位 / 破图图标 | ⛔ | 加载失败不绘制可见 alt |
| CSS `@import` | ✅🆕 | 递归抓取 + 原位内联(深度 8 层,循环引用跳过) |
| `@import` 的 `media` 条件 | ✅🆕 | 内联时包成 `@media` 块 |
| `@import layer()` / `supports()` | ⛔ | 仍按未知 at-rule 跳过 |
| `@font-face` / `@keyframes` | ⛔ | 样式引擎跳过(字体不加载) |
| SVG 子集光栅化 | ⛔ | 内联 `<svg>` 只有固有尺寸,子元素不画 |
| video/audio/poster | ⛔ | 仅 DOM 外壳 |
| iframe 子文档 | ⛔ | 300×150 占位 |

---

## 四、显示异常排查映射

| 现象 | 优先检查 |
| --- | --- |
| 图片位置空一块,但 DOM 有 `<img>` | 该 `<img>` 是否只有 `srcset`;`collect_image_urls` 是否收集到候选 |
| `<picture>` 的宽屏图没出现 | `picture_srcset` 的 `media` 条件是否被解析(`min-width`/`max-width` px) |
| 选到了 1x 图但想要 2x | 内核 DPR 恒为 1,`x` 描述符按 1 选 |
| 站点样式整体丢失 | 站点 CSS 是否用 `@import`;`inline_css_imports` 是否抓取到依赖表 |
| 导入表抓取失败 | console 的「@import 样式加载失败」warn;网络台账里的 404 |
| 导入的规则没生效 | 被导入表是否在 `@import` 之后出现(CSS 规范要求 `@import` 在规则之前) |

---

## 五、测试矩阵

| 层 | 文件 | 本轮覆盖 |
| --- | --- | --- |
| 文档(纯函数) | `src/agent/browser/document.rs::tests` | `srcset` 候选解析、`x`/`w` 描述符选择、`<picture>` 的 `source` 命中 |
| 图片预取 | `src/agent/browser/images.rs::tests` | `collect_image_urls` 含 `srcset` 候选 |
| 端到端 | `tests/web_resource_srcset.rs`(新) | `@import` 内联后规则参与计算样式;`srcset` 按密度选候选并预取、2x 候选**不**被预取 |

验证命令:

```bash
cargo test --lib document::
cargo test --test web_resource_srcset
cargo test --no-fail-fast
./rebuild_restart_app.sh
```

---

## 六、后续路线

1. **DPR 参与渲染**:候选选择、Canvas 位图缓冲、`image-rendering` 一起接入 `device_scale_factor`。
2. `sizes` 属性参与 `w` 描述符的源尺寸计算(元素渲染宽度而非视口宽)。
3. `@import layer()` / `supports()` / `@font-face` 的 at-rule 支持。
4. 图片失败占位:绘制可见 alt 文本与破图图标。
5. `<picture>` 的 `type` 精细匹配与 `<source media>` 的完整媒体查询求值。
6. SVG 子集光栅化、video poster、iframe 子文档。
7. 内部滚动容器(元素级 `overflow:auto/scroll` + 滚动条)。
8. 行内块独立格式化上下文(盒宽包住自己的文字)。
9. 完整 stacking context、CSS 动画帧同步、性能(脏子树/样式缓存/显示列表 diff)。
