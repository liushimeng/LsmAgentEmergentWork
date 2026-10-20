# 阶段二第二步:CSS 级联引擎与外链样式加载

> 对应总体路线图 Phase 2 第二步(`01-LsmAgentBrowser总体设计与分阶段路线图.md`)。
> 目标:**样式从"能解析一部分"升级为"可复算的级联系统",并补齐外链样式表资源加载**,
> 为阶段三布局/渲染、阶段五 Web API 提供统一的计算样式真源。
> 状态:✅ 基础框架已落地(实现状态见文末"九、实现状态")。

## 一、现状与问题

阶段一/二第一步的样式能力散落在 `browser/layout.rs`:

| 现状 | 缺陷 |
| --- | --- |
| 只解析 `<style>` 与元素 `style` 属性 | `<link rel="stylesheet">` 外链样式完全不加载 |
| 选择器按空白切分,`>` 被丢弃 | `.a > .b` 与 `.a .b` 语义相同(错误) |
| 特异性只有 `id*100+class*10+tag` 单一数值 | `!important`、来源顺序、内联样式优先级无法正确参与级联 |
| 无继承 | `body{color:red}` 对子元素无效 |
| 只认 px | `em/rem/%/pt` 与 `small/large` 等关键字被丢弃 |
| 无 `@media` | 响应式站点按桌面样式硬算 |
| 无自定义属性 | `var(--x)` 站点样式大面积失效 |
| `visibility:hidden` 被当作 `display:none` | 隐藏元素仍应占据布局空间 |
| `text-align` 不生效 | 居中/右对齐页面截图与坐标偏移 |
| JS `getComputedStyle` 只读内联样式 | 依赖计算样式的页面脚本判断错误 |

本轮把样式能力收敛为**独立样式引擎**,布局/渲染/JS/观测全部消费同一份计算样式。

## 二、目标与验收

### 2.1 目标

1. **外链样式加载**:`<link rel="stylesheet" href>` 由宿主预取并内联为 `<style>`(保持文档顺序),
   记录网络台账;失败降级不阻断页面。
2. **正确级联**:来源(UA → 作者 → 内联)+ 特异性(四位元组)+ `!important` + 声明顺序。
3. **继承与单位**:可继承属性沿 DOM 链继承;`px/em/rem/%/pt/关键字` 解析。
4. **选择器完善**:后代 / 子代 `>` / 相邻 `+` / 兄弟 `~`,以及
   `:first-child/:last-child/:nth-child/:nth-of-type/:first-of-type/:last-of-type/:not()/:root/:empty/:checked/:disabled/:enabled/:link`。
5. **媒体查询子集**:`@media` 与 `<link media>`;`screen/all/print`、`min/max-width`、`min/max-height`、
   `orientation`、`prefers-color-scheme`、`and`/逗号/`not`。
6. **自定义属性**:`--var` 声明、继承、`var(--x, fallback)` 替换(深度上限 8)。
7. **计算样式可观测**:`getComputedStyle(el)` 返回真实计算值;`inspect(info="stylesheets")` 输出样式表清单;
   `inspect(info="elements", params.include_style=true)` 输出元素计算样式。
8. **布局消费**:字号/颜色/背景/边距/内边距/行高/`text-align`/`visibility`/`white-space` 参与布局与渲染。

### 2.2 验收

- 单测覆盖级联/继承/单位/选择器/媒体查询/自定义属性/外链内联;
- 集成测试:mock HTTP 站点通过外链 CSS 隐藏元素、改变字号/颜色,`inspect` 与截图结果可判定;
- `cargo test --lib`、`cargo test --test web_use_engine` 全过;
- `./rebuild_restart_app.sh` 构建通过,真实二进制冒烟可读依赖外链 CSS 的页面。

## 三、总体架构

```
                    ┌──────────────────────────────────────────────┐
 导航取主文档 ──────▶│ resources::load_stylesheets                  │
                    │  1. 收集 <link rel=stylesheet>               │
                    │  2. 宿主 HttpClient 预取(Cookie/SSRF/TLS 同源)│
                    │  3. 原位替换为 <style data-lsm-href=...>      │
                    │  4. 网络台账 + 失败降级                       │
                    └───────────────────┬──────────────────────────┘
                                        ▼
                    ┌──────────────────────────────────────────────┐
                    │ script::run_pipeline(页面脚本,可改 DOM)        │
                    └───────────────────┬──────────────────────────┘
                                        ▼
   ┌────────────────────────────────────────────────────────────────┐
   │ style::StyleEngine                                              │
   │  parse_stylesheets → Rule{Selector, Decl, Media, Order}         │
   │  computed(node, parent) → ComputedStyle                         │
   │    UA 默认 → 继承 → 作者声明(特异性/!important)→ 内联           │
   └───────┬──────────────────────┬──────────────────────┬──────────┘
           ▼                      ▼                      ▼
   layout::compute          JS snapshot(css 字段)     inspect(elements/stylesheets)
   块级布局/对齐/可见性       getComputedStyle         计算样式可观测
           ▼
   render::render(跳过 visibility:hidden)
```

核心决策:

| 决策 | 理由 |
| --- | --- |
| **外链 CSS 在宿主异步预取,不在 JS 线程** | 复用 `HttpClient` 的 Cookie/重定向/字符集/SSRF/TLS 策略,与主文档、外链脚本一致 |
| **原位替换 `<link>` 为 `<style>`** | 保持文档顺序,级联中"后出现者胜"与真实浏览器一致;避免追加到 head 尾部导致顺序错乱 |
| **样式引擎输出纯数据 `ComputedStyle`** | 不持有 DOM 句柄,可跨 await;布局/渲染/JS 快照共用一份真源 |
| **`Document` 仍是短生命周期局部值** | 维持 `TabState: Send`;样式规则在单次同步操作内解析,不跨 await |
| **媒体查询只做子集** | 覆盖响应式站点常见断点;未识别特性按不匹配处理,避免误用桌面/打印样式 |
| **自定义属性做简化实现** | 支持主流 `var()` 用法;不实现 `@property`/类型化自定义属性 |

## 四、模块划分

```
src/agent/browser/
  style/
    mod.rs       StyleEngine / Rule / 样式表收集 / @media 解析 / 级联主流程
    selector.rs  选择器 AST、解析、匹配、特异性
    values.rs    ComputedStyle、长度/颜色/关键字解析、声明应用、var() 替换
    tests.rs     样式引擎单元测试
  resources.rs   外链样式表收集、预取、原位内联、台账
  layout.rs      消费 ComputedStyle(不再自带 CSS 解析)
  document.rs    SnapshotNode 增加 css 字段;样式感知的快照遍历
```

### 4.1 关键类型

```rust
pub struct MediaCtx {
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub prefers_dark: bool,
    pub print: bool,
}

pub struct StyleEngine {
    rules: Vec<Rule>,          // 文档序;order 单调递增
    media: MediaCtx,
    sheets: Vec<SheetInfo>,    // 观测用:href/media/字节/规则数
    root_font_size: f64,
}

pub struct ComputedStyle {
    pub display: String,
    pub visibility: String,
    pub color: [u8; 3],
    pub background: Option<[u8; 3]>,
    pub font_size: f64,
    pub font_weight: u16,
    pub font_family: String,
    pub line_height: f64,      // px;normal 折算为 1.45×font-size
    pub text_align: String,
    pub white_space: String,
    pub margin_top/right/bottom/left: f64,
    pub padding_top/right/bottom/left: f64,
    pub width/max_width/height: Option<f64>,
    pub opacity: f64,
    pub extra: BTreeMap<String, String>, // 未建模属性的计算文本
}
```

### 4.2 级联主流程

```
computed(node, parent):
  1. base = ua_default(tag, parent)        // 继承可继承属性 + 标签默认值
  2. decls = 所有命中规则 + 内联样式        // 过滤 @media 不匹配
  3. 分离自定义属性声明(--x) → 继承父级 → 级联排序 → 得到 var 表
  4. 其余声明 var() 替换后按
       (important, origin, specificity, order) 升序应用
  5. 元素级硬约束:hidden / aria-hidden=true / input[type=hidden] → display:none
  6. 产出 ComputedStyle + to_map() 供 getComputedStyle
```

优先级键:

| 维度 | 取值(低 → 高) |
| --- | --- |
| important | 普通 0 → `!important` 1 |
| origin | UA 0 → 作者 1(内联属于作者来源) |
| specificity | `(a,b,c)`;内联视作 `(1,0,0,0)` |
| order | 文档出现顺序,后者覆盖前者 |

### 4.3 选择器

- **组合符**:后代(空白)、子代 `>`、相邻兄弟 `+`、一般兄弟 `~`;从右向左匹配;
- **复合选择器**:标签 / `*` / `#id` / `.class` / `[attr]` / `[attr=v]` /
  `[attr~=v]` / `[attr^=v]` / `[attr$=v]` / `[attr*=v]` / `[attr|=v]`;
- **伪类**:上节目标清单;`:hover/:focus/:active` 等动态伪类恒不匹配(无交互态建模);
- **伪元素** `::before/::after` 的规则整条跳过(不生成伪元素盒);
- **非法选择器**只丢弃该选择器,不影响同一规则组中其它选择器。

### 4.4 值与单位

| 类别 | 支持 |
| --- | --- |
| 长度 | `px`、`em`(相对当前元素字号,`font-size` 相对父级)、`rem`(根 16px)、`%`、`pt`(×4/3)、无单位数值 |
| 字号关键字 | `xx-small/x-small/small/medium/large/x-large/xx-large`、`smaller/larger` |
| 行高 | 无单位倍数、px、em、%、`normal` |
| 颜色 | 命名色(常用 40+)、`#rgb/#rgba/#rrggbb/#rrggbbaa`、`rgb()/rgba()`、`hsl()/hsla()`、`transparent`、`currentColor` |
| 关键字 | `inherit` / `initial` / `unset` / `revert`(按 initial 处理) |
| 自定义属性 | `--name: value`、`var(--name[, fallback])`,替换深度 ≤ 8 |
| 百分比 | 宽度/最大宽度相对视口宽;内外边距相对视口宽(简化,文档化边界) |

### 4.5 媒体查询

- 语法:`@media <query-list> { ... }`、`<link media="...">`;
- 媒体类型:`all`、`screen` 匹配(非打印);`print` 不匹配;
- 特性:`(min-width: N)`、`(max-width: N)`、`(min-height: N)`、`(max-height: N)`、
  `(orientation: landscape|portrait)`、`(prefers-color-scheme: dark|light)`;
- 组合:逗号 = OR;`and` = AND;`not` 否定整条;
- 未识别特性按**不匹配**处理(避免把打印/触屏专用样式误用到桌面布局)。

## 五、外链样式加载(`resources.rs`)

```rust
pub struct StyleLink { pub node_selector: String, pub href: String, pub media: String }

pub struct ResourceOutcome {
    pub html: String,
    pub network: Vec<NetworkRecord>,
    pub console: Vec<ConsoleRecord>,
    pub stylesheets: Vec<LoadedSheet>, // url/media/bytes/status/source
    pub skipped: Vec<String>,
}

pub fn styles_enabled() -> bool;                    // LAEW_WEB_STYLES=off 关闭
pub fn collect_stylesheet_links(html, base) -> Vec<StyleLink>;
pub fn apply_fetched_styles(html, base, fetched) -> String;
pub async fn load_stylesheets(inner, http, html, base, ua, timeout_ms) -> ResourceOutcome;
```

规则:

1. 仅处理 `rel` 含 `stylesheet`(大小写不敏感)且 `href` 非空的 `<link>`;
2. 支持 `data:text/css,...` 直接内联,不发起网络;
3. 每页最多 `LAEW_WEB_MAX_STYLESHEETS`(默认 16)个,单个 ≤ 2MB、总量 ≤ 4MB;
4. 失败(网络/非 2xx/过大)记 `console.warn` 与台账,保留页面继续加载;
5. `media` 属性非空时把 CSS 包进 `@media <attr> { ... }`;
6. 原位替换 `<link>` 为 `<style data-lsm-href="<绝对 URL>">`,保持文档顺序;
7. 无外链样式时**原样返回 HTML**,零额外解析成本。

## 六、布局 / 渲染 / JS / 观测集成

### 6.1 布局(`layout.rs`)

- 删除自有 CSS 解析/匹配/单位代码,统一调用 `StyleEngine`;
- 新增 `compute_with(doc, viewport, font, media)`;原 `compute(...)` 保持签名不变;
- `text-align: left/center/right` 影响文本行 x 偏移;
- `visibility: hidden` 保留布局空间,文本行/元素盒标记 `visible=false`;
- `white-space: pre/pre-wrap` 保留空白与换行;
- `display:none` 及祖先隐藏继续不参与布局。

### 6.2 渲染(`render.rs`)

- 跳过 `visible=false` 的文本行与元素盒背景;
- 盒子背景/文字颜色来自计算样式(已有通道,语义不变)。

### 6.3 JS(`script/prelude.rs`)

- 快照节点增加 `css` 计算样式表;
- `getComputedStyle(el)` 读取快照计算样式,支持 `getPropertyValue()` 与直接属性访问;
- `matchMedia(query)` 按视口尺寸/方向/配色偏好求值,返回 `matches` 与监听空实现。

### 6.4 观测(`inspect.rs`)

| info / 参数 | 新增输出 |
| --- | --- |
| `inspect(info="stylesheets")` | `{ok,count,rules,sheets:[{href,media,bytes,rules}],media:{width,height}}` |
| `inspect(info="elements", params.include_style=true)` | 元素附加 `computed_style:{...}`(常用属性) |
| `inspect(info="elements")` 既有字段 | 不变(向后兼容) |

## 七、环境变量与预算

| 变量 | 取值 | 默认 | 行为 |
| --- | --- | --- | --- |
| `LAEW_WEB_STYLES` | `off/0/false/no` | 开启 | 关闭外链样式加载(内联 `<style>` 仍生效) |
| `LAEW_WEB_MAX_STYLESHEETS` | 正整数 | 16 | 单页外链样式上限 |
| `LAEW_WEB_MAX_CSS_BYTES` | 正整数 | 2097152 | 单个样式表字节上限 |
| `LAEW_WEB_PREFERS_DARK` | `1/true/yes/on` | 关闭 | `prefers-color-scheme: dark` 求值 |

安全:样式请求沿用主机网络栈的 SSRF/TLS/Cookie 策略;不执行 CSS 中的 `url()`/脚本;
`@import` 不递归抓取(阶段四资源加载器统一处理),避免样式导入链放大请求。

## 八、测试策略

| 层 | 用例 |
| --- | --- |
| 单元 `style/tests.rs` | 特异性排序、`!important`、内联覆盖、继承、`>` vs 后代、`+`/`~`、`nth-child(2n+1)`、`:not`、`em/rem/%/pt`、媒体查询、`var()`、`visibility:hidden` 保留空间、`text-align` |
| 单元 `resources.rs` | 链接收集、`data:` CSS、原位替换顺序、`media` 包装、失败降级 |
| 单元 `layout.rs` | 外链样式内联后字号/颜色生效、隐藏元素 |
| 集成 `tests/web_use_engine.rs` | mock 站点:外链 CSS 隐藏元素/改变字号;`inspect(stylesheets)`;`include_style` 返回计算样式 |
| 构建 | `cargo test --lib`、`cargo test --test web_use_engine`、`./rebuild_restart_app.sh` |

## 九、实现状态

> 状态:✅ 已实现;🟡 部分实现;⏳ 后续阶段。

| 能力 | 状态 | 代码 |
| --- | --- | --- |
| 独立样式引擎(规则/特异性/级联/`!important`) | ✅ | `browser/style/mod.rs` |
| 选择器组合符与伪类子集 | ✅ | `browser/style/selector.rs` |
| 继承 / 单位 / 颜色 / `var()` | ✅ | `browser/style/values.rs` |
| `@media` 与 `<link media>` 子集 | ✅ | `browser/style/mod.rs` |
| 外链样式预取与原位内联 | ✅ | `browser/resources.rs` |
| 布局消费计算样式(text-align / visibility / white-space) | ✅ | `browser/layout.rs` |
| `getComputedStyle` / `matchMedia` | ✅ | `browser/script/prelude.rs` + 快照 `css` |
| `inspect(stylesheets)` / `elements.include_style` | ✅ | `browser/inspect.rs` |
| `@import` 递归 | ⏳ | 阶段四资源加载器 |
| 完整 CSS 层叠(动画/过渡/伪元素/`@supports`) | ⏳ | 阶段三/五 |
| Flex/Grid/定位布局 | ⏳ | 阶段三 |
| 图片/字体等子资源加载与渲染 | ⏳ | 阶段四 |

## 十、已知边界

1. **伪元素不渲染**:`::before/::after` 规则跳过,装饰性内容缺失;
2. **百分比边距/宽度按视口宽估算**,未按包含块精确计算;
3. **`@import` 不递归**,依赖导入的站点样式可能不完整;
4. **动态伪类不匹配**:`:hover/:focus/:active` 规则不参与级联;
5. **自定义属性无类型/无 `@property`**,`var()` 仅在声明值中替换;
6. **媒体查询未覆盖**数值范围语法(`width >= 600px`)、`resolution`、`hover/pointer`;
7. **CSS 变量在 JS 运行时新增的 `style.setProperty('--x')`** 只落内联样式,下一次布局重新解析时生效(与现有 DOM 回写模型一致)。
