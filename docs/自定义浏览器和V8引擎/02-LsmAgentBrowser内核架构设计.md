# LsmAgentBrowser 内核架构设计

## 一、模块布局

```
src/agent/browser/
  mod.rs          进程级单例 LsmBrowserManager、Window/Tab 模型、page_id 生命周期
  api.rs          control_action 自动化实现（点击/输入/选择/按键/滚动/JS/视口）
  inspect.rs      inspect info 观测实现（elements/dom/network/extract 等）
  net.rs          HTTP(S) 网络栈、重定向、Cookie、字符集、请求台账
  document.rs     html5ever/kuchikiki DOM 封装、选择器、稳定选择器、文本抽取、表单
  layout.rs       块级布局、文本换行、CSS 子集、元素矩形、页面尺寸
  render.rs       软件光栅化（tiny-skia + fontdue）与 PNG 编码
  js.rs           JsRuntime 抽象、Boa 后端、DOM 快照桥
  storage.rs      Cookie Jar、localStorage、sessionStorage（全内存）
  urlutil.rs      URL 解析、相对地址、字符集解码、文件名净化
  tests.rs        内核单元测试
```

兼容层仍在 `src/agent/tools/mcp_web_use/`，只依赖 `browser::api`，不直接接触 DOM/网络。

## 二、对象模型

### 2.1 Window / Tab

```rust
pub struct WindowState {
    pub id: String,          // "w1"
    pub tabs: Vec<String>,   // TabId 顺序
    pub active: Option<String>,
    pub width: u32,
    pub height: u32,
    pub created_at: u64,
}

pub struct TabState {
    pub id: String,          // "p_00ab12cd"
    pub window_id: String,
    pub url: String,         // 当前 URL
    pub title: String,
    pub dom: Document,
    pub history: Vec<HistoryEntry>,
    pub history_index: usize,
    pub storage: TabStorage,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub viewport: Viewport,
    pub status: u16,
    pub content_type: String,
    pub source_bytes: usize,
    pub network: Vec<NetworkRecord>,
    pub console: Vec<ConsoleRecord>,
    pub form_values: HashMap<String, String>, // 选择器 → 当前值
    pub js: Option<Box<dyn JsRuntime>>,
    pub created_at: u64,
    pub last_active: u64,
    pub foreground: bool,
}
```

- **page_id**：`p_` + 8 位十六进制（`p_00ab12cd`），进程内单调递增；`close` 后不可复用。
- **窗口**：`open(new_window=true)` 创建新窗口；否则加入当前窗口的新 Tab。
- **前台**：每个窗口最多一个前台 Tab；`bring_to_front` 切换。

### 2.2 单例与并发

```rust
pub struct LsmBrowserManager {
    inner: tokio::sync::Mutex<Inner>,
    http: reqwest::Client,
}
pub fn global() -> Arc<LsmBrowserManager>   // once_cell
```

- 全局状态由 `Mutex<Inner>` 保护，临界区只做内存操作；
- 网络请求在锁外执行，完成后重新加锁提交结果；
- 锁内不 `.await` 网络/睡眠，避免头阻塞；`wait` 在锁外 sleep 后复查；
- 单个 Tab 的操作以 page_id 定位，不持有跨请求引用。

## 三、网络栈

### 3.1 请求

```rust
pub struct FetchRequest {
    pub url: String,
    pub method: Method,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: u64,
}
pub struct FetchResponse {
    pub final_url: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub bytes: Vec<u8>,
    pub redirect_chain: Vec<String>,
    pub elapsed_ms: u64,
}
```

- `reqwest::Client` 关闭自动重定向（`Policy::none`），手动跟随最多 10 跳；
- 每跳解析 `Set-Cookie` 写入 Jar，并对下一跳带 `Cookie`；
- 超时：默认 60s（`open` 的 `timeout_ms`），上限由调用方 clamp；
- TLS：沿用工程 `LAEW_TLS_INSECURE` 语义；私网/环回沿用 `LAEW_ALLOW_PRIVATE_ENDPOINT`；
- User-Agent 缺省：`Mozilla/5.0 (compatible; LsmAgentBrowser/0.1; +https://localhost/laew)`，可由 `user_agent` 覆盖；
- 解压：启用 reqwest `gzip`/`brotli`/`deflate`；
- 字符集：优先 `Content-Type; charset=`，其次 HTML `<meta charset>`/`http-equiv`，用 `encoding_rs` 解码；缺省 UTF-8 lossy；
- 只加载主文档：阶段一不自动抓取 `<img>/<link>/<script>` 子资源（阶段二引入资源加载器）。

### 3.2 请求台账

每 Tab 记录最近 200 条：`{url, method, status, content_type, bytes, elapsed_ms, redirect}`。
供 `inspect(info="network")` 使用。

## 四、DOM 与 CSS

### 4.1 DOM

- `Document::parse(html, base_url)` → `kuchikiki::NodeRef`；
- 查询：`select(selector) -> Vec<NodeRef>`；选择器语法由 `kuchikiki` 支持（CSS3 子集）；
- 稳定选择器生成（`stable_selector(node)`）：
  1. 有唯一 `id` → `#id`；
  2. 否则从节点向上构造 `tag:nth-of-type(n)` 路径；
  3. 同名同类型兄弟 ≥2 时追加 `:nth-of-type`。
- 文本抽取：遍历文本节点，跳过 `script/style/noscript/template`，块级元素之间插入换行；
- 可见性：阶段一以 `display:none`/`hidden`/`aria-hidden="true"`/`type=hidden` 为不可见判据；
- 表单：`input/textarea/select/button`；值优先取运行时覆盖表（`form_values`），其次 `value`/`checked`/`selected`。

### 4.2 CSS

阶段一解析并应用以下子集：

- 来源：`<style>` 内容、`<link rel=stylesheet>` 已内联时不抓取（阶段四）、元素 `style` 属性；
- 选择器：标签、`.class`、`#id`、`[attr]`、`[attr=value]`、后代/子代、逗号分组；
- 声明：`display`、`visibility`、`color`、`background-color`、`font-size`、`font-weight`、`font-family`、
  `line-height`、`margin*`、`padding*`、`border*`、`width/height`、`max-width`、`text-align`、`white-space`；
- 计算顺序：UA 默认值 → 作者样式（按出现顺序，同权重后者覆盖）→ 内联样式；
- 阶段一不做完整层叠/继承/媒体查询（阶段二/三补齐），但提供稳定的 `computed_style(node)` 供布局与渲染使用。

## 五、布局与渲染

### 5.1 布局（阶段一）

- 视口缺省 `1280×720`，可由 `set_viewport` 调整（320..7680 × 240..4320）；
- 块级流式布局：`block/flex/grid/table` 一律按块级处理；`inline/inline-block` 参与文本行；
- 文本换行：按字体度量测量宽度，超过内容宽度折行；`white-space: pre` 保留换行；
- 产出：
  - `Vec<TextLine>`：`{text, x, y, font_size, color, weight}`；
  - `Vec<ElementBox>`：`{selector, tag, x, y, w, h}`（近似矩形，供 `inspect(elements)` / 坐标点击）；
  - `page_height`；
- 阶段三替换为完整 Flex/Grid/定位布局，接口不变。

### 5.2 渲染

- `Renderer::render(tab, region, full_page) -> Vec<u8>`（PNG）；
- 背景白色；按 `ElementBox` 绘制背景色/边框（简化）；按 `TextLine` 用 `fontdue` 栅格化字形，
  `tiny-skia` 合成到 Pixmap；
- 字体发现：
  - macOS：`/System/Library/Fonts/Supplemental/Arial Unicode.ttf` → `Arial.ttf`
  - Windows：`C:\Windows\Fonts\msyh.ttc` → `segoeui.ttf` → `arial.ttf`
  - Linux：`/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf` → Noto Sans CJK
  - 全部失败：只绘制色块与占位线，响应附 `render_note`；
- 截图落盘：缺省 `<artifact_root>/web_screenshots/web_{ts}_{page_id}.png`，也接受 `save_path`；
- `return_base64=true` 时附带 base64（默认不返回）；
- `ocr=true`：阶段一返回布局文本（`text` 字段）+ 词块（按行切分），不是真实 OCR；字段名与旧契约兼容。

## 六、JS 引擎

### 6.1 抽象

```rust
pub trait JsRuntime: Send {
    fn eval(&mut self, script: &str, timeout_ms: u64) -> Result<JsOutcome, String>;
    fn reset(&mut self);
}
pub struct JsOutcome { pub value: String, pub value_type: String, pub pending: Vec<DomAction> }
```

### 6.2 Boa 后端（阶段一）

- 每个 Tab 一个 `boa_engine::Context`；
- 每次执行前注入 DOM 快照前导脚本：
  - `document.title / URL / documentElement / body`；
  - `document.querySelector / querySelectorAll / getElementById / getElementsByTagName`（JS 侧匹配快照）；
  - 元素对象：`tagName/id/className/textContent/innerText/innerHTML/value/checked/selectedIndex`、
    `getAttribute/setAttribute/removeAttribute/click()`；
  - `location.href`；`window.innerWidth/innerHeight/scrollX/scrollY`；
- 元素 `click()` 与 `location.href = ...` 写入 `__lsm_actions`，执行后由宿主应用：
  - `navigate(url)`：导航；
  - `click(selector)`：原生点击语义；
  - `set_value(selector, value)`：写入运行时值；
- 返回值：`undefined`→`"undefined"`；对象/数组 JSON 化；超长（>256KB）落盘并返回 `saved_to`；
- 超时：`LAEW_WEB_EVAL_TIMEOUT_MS`（默认 30s）；Boa 无中断 API，采用执行前限制脚本长度 + 执行后校验，
  阶段二接入 V8 后可实现真正中断。

### 6.3 V8 后端（后续）

- `rusty_v8` + `HandleScope/Context`；
- DOM 绑定改为原生回调（非快照），支持真正的动态 DOM；
- 通过 Cargo 特性 `v8-backend` 启用；上层只依赖 `JsRuntime`。

## 七、存储

| 存储 | 作用域 | 实现 |
| --- | --- | --- |
| Cookie | 会话级（跨 Tab） | `CookieJar { domain, path, name, value, expires, secure, http_only, same_site }` |
| localStorage | 进程级 | `HashMap<origin, HashMap<key, value>>` |
| sessionStorage | 标签页级 | `HashMap<key, value>` |
| 表单值 | 标签页级 | `HashMap<stable_selector, value>` |
| 下载文件 | 内存 + 可选落盘 | `Vec<DownloadRecord>` |

Cookie 匹配：域名后缀匹配 + 路径前缀匹配 + `Secure` 与当前 scheme 一致；`HttpOnly` 仅阻止 JS 读取（快照不暴露）。

## 八、交互语义

| 动作 | 阶段一实现 |
| --- | --- |
| click | `<a href>` → 导航；`<button>/<input type=submit>` → 表单提交；其余元素 → 记录点击事件并返回 |
| right_click / double_click | 同 click（附带事件类型标记） |
| hover | 记录 hover 目标，返回元素矩形 |
| input_text | 写表单值；`contenteditable` 写文本节点 |
| clear_input | 清空值 |
| key_press | `Enter` 在输入框内触发所在表单提交；`Escape` 停止加载；其余按键记录 |
| press_sequence | 顺序执行按键 |
| select_option / select_dropdown | 按 value/label/索引设置 `selected` |
| scroll / scroll_to / scroll_to_text | 更新滚动偏移；`scroll_to_text` 在 DOM 文本中查找并定位 |
| navigate / back / forward / reload | 历史栈操作 |
| new_tab / new_window / close_tab / bring_to_front | 窗口/Tab 操作 |
| wait | 纯等待；带 selector 时轮询 DOM 直到命中或超时 |
| eval_js | JS 运行时执行 |
| set_cookie / delete_cookie / set_storage / clear_storage | 存储操作 |
| set_viewport | 调整视口并重排 |
| screenshot | 渲染 PNG |
| download | HTTP 下载落盘 |
| dispatch_event | 记录事件并在需要时触发点击/输入语义 |
| focus / blur / mouse_move | 记录焦点/指针状态 |
| upload_file | 记录文件路径到表单值（阶段一不读取文件内容） |
| request_human | 桥接 `human_assist`（见下） |
| drag | 起点/终点记录，阶段三实现真实拖拽 |
| set_window / sync_viewport | 调整逻辑视口并重排 |
| set_highlight / set_overlay / set_guard / auto_follow / set_stealth / heartbeat | 接受参数，返回 `{supported:false, phase:...}` 说明；不改变页面语义 |

## 九、安全策略

- 导航 URL 必须为 `http/https/file/data/about`；`file:` 仅限工作目录内（阶段一仅允许显式路径）；
- 私网/环回地址默认拦截（沿用 SSRF 防护）；`LAEW_ALLOW_PRIVATE_ENDPOINT=1` 放行；
- 任务锚点（`safety::target_anchor`）在 `open/navigate/new_tab/new_window` 入口继续生效，越界返回 6001；
- 不执行页面脚本（阶段一），天然规避大部分 XSS；阶段二引入脚本后补同源/CSP；
- 下载：单文件缺省上限 256MB（`LAEW_WEB_MAX_DOWNLOAD_MB` 可调），文件名净化，拒绝路径穿越。

## 十、错误模型

| 码 | 语义 | 新引擎来源 |
| --- | --- | --- |
| 0 | 成功 | — |
| 1001 | 参数错误 | 缺 action/url/page_id、非法枚举、互斥参数 |
| 2000 | page_id 失效 | Tab 已关闭/未创建 |
| 2001 | 页面断连 | 导航失败/网络错误（可重试） |
| 2002 | 动作失败 | 选择器未命中、元素不可交互、JS 异常、超时 |
| 2003 | 页面崩溃 | 内核内部错误（保留） |
| 3001 | 环境缺失 | 字体/渲染不可用等 |
| 3002 | 能力不可用 | `reuse_existing`/`connect_url`（新引擎无外部浏览器） |
| 4001 | 人工介入不可用 | HITL 无可用通道/超时 |
| 4002 | 人工取消任务 | 人工点「取消任务」/关窗 |
| 4003 | 人工介入进行中 | 页面归人工所有 |
| 6001 | 任务锚点越界 | 目标站外导航 |
| 6002 | 人工核验输入闸 | 阶段一不启用（保留语义） |
| 6003 | 后台标签页限制 | 阶段一所有 Tab 均可前台化，保留码 |
| 6004 | 站点节奏冷却 | 阶段一不启用（保留码） |

## 十一、测试策略

- 单元测试（`src/agent/browser/tests.rs`）：URL 解析、Cookie 匹配、字符集、选择器、稳定选择器、表单序列化、文本抽取、布局换行、渲染 PNG 头、JS 桥动作；
- 集成测试（`tests/web_use_engine.rs`）：进程内 mock HTTP 服务（真实 TCP/HTTP），覆盖：
  - open → title/text/elements；
  - click 链接 → 导航；
  - input_text + key_press(Enter) → GET 表单提交；
  - sequence 多步 + stop_on_error；
  - screenshot 落盘；
  - close(all)；
- 不使用真实外网，不依赖 Chrome，不依赖 CDP。
