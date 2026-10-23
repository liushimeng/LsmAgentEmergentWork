# LsmAgentBrowser 总体设计与分阶段路线图

## 一、目标与硬约束

### 1.1 目标

用自研浏览器内核 **LsmAgentBrowser** 替换 `MCP_Web_LsmAgentBrowser_Use` 工具原有的 Chrome + CDP 实现：

- 浏览器与 Agent 在**同一进程**内运行，不存在外部浏览器进程、调试端口、WebSocket/管道远程协议；
- 页面能力（导航、DOM、选择器、表单、Cookie、存储、JS 执行、截图、下载）由内核直接提供；
- `MCP_Web_LsmAgentBrowser_Use` 的**工具名、action 枚举、参数形状、响应信封、错误码语义保持不变**；
- 删除全部 Chrome/CDP 相关代码、依赖、环境变量与文档。

### 1.2 硬约束

| 约束 | 说明 |
| --- | --- |
| 无 Chrome | 不启动、不探测、不复用本机 Chrome/Edge/Chromium/Brave |
| 无 CDP | 不引入 `chromiumoxide`，不实现 DevTools 远程调试客户端 |
| 无外部浏览器进程 | 不复制用户浏览器 profile，不占用调试端口 |
| 契约不变 | Agent 侧看到的 `MCP_Web_LsmAgentBrowser_Use` 调用方式与错误码不变 |
| 跨平台 | macOS / Windows / Linux 同一套 Rust 代码，平台差异只允许出现在字体/路径探测 |
| 可编译 | 默认特性离线可编译；重量级引擎（V8）作为可选特性 |

## 二、技术选型

### 2.1 总体栈

| 层 | 选型 | 说明 |
| --- | --- | --- |
| 语言/运行时 | Rust + Tokio | 与 `laew` 主工程一致 |
| HTML 解析 / DOM | `html5ever` + `kuchikiki` | 可变 DOM 树 + CSS 选择器查询/修改 |
| CSS 解析 | 自研轻量级解析器 | 解析 `<style>` / `style` 属性：选择器、颜色、字体、盒模型、display 等子集 |
| 网络 | `reqwest`（rustls） | HTTP/1.1、HTTP/2、重定向、Cookie、表单提交 |
| JS 引擎（默认引导后端） | `boa_engine` | 纯 Rust，离线可编译，保证三平台开箱可用 |
| JS 引擎（目标后端） | `rusty_v8`（可选特性） | 工业级 V8 绑定；预编译产物需从 GitHub 获取，受网络条件约束 |
| 2D 渲染 | `tiny-skia` + `fontdue` | 纯 Rust 软件光栅化，无 GPU 依赖 |
| 图片编码 | `image`（工程既有依赖） | 截图输出 PNG |
| 字体发现 | 平台候选路径 + `Arial Unicode` 等 | 中文优先选择系统 Unicode 字体 |
| 持久化 | 全内存 | 进程退出即销毁；不写磁盘缓存/配置 |

### 2.2 关于 V8

`rusty_v8` 的构建脚本需要下载与版本精确匹配的预编译静态库（默认来自 GitHub Releases）。
当前构建环境无法访问 GitHub，因此**阶段一默认使用 `boa_engine` 作为引导后端**，同时：

- 内核通过 `JsRuntime` trait 隔离 JS 引擎；
- 提供 `v8-backend` 可选 Cargo 特性（`rusty_v8`）；
- 在可获取预编译产物的环境中，`cargo build --features v8-backend` 即可切换，不改上层代码。

> 这是构建可用性约束下的工程取舍，不是最终形态。V8 是目标后端，Boa 是离线引导后端。

## 三、总体架构

```
┌──────────────────────────────────────────────────────────────┐
│ Agent / LLM                                                   │
│   └─ MCP_Web_LsmAgentBrowser_Use(action/control_action/info/params)           │
├──────────────────────────────────────────────────────────────┤
│ 兼容层 src/agent/tools/mcp_web_use/                            │
│   参数归一化 · action 分发 · 信封/错误码 · sequence/batch      │
├──────────────────────────────────────────────────────────────┤
│ 自动化 API src/agent/browser/api.rs                           │
│   open/navigate/click/type/scroll/inspect/screenshot/...       │
├──────────────────────────────────────────────────────────────┤
│ LsmAgentBrowser 内核 src/agent/browser/                        │
│  ┌─────────┬─────────┬─────────┬─────────┬─────────┐          │
│  │ Tab 管理 │ 网络栈  │ DOM/CSS │ 布局渲染 │ JS 引擎 │          │
│  ├─────────┼─────────┼─────────┼─────────┼─────────┤          │
│  │ 存储    │ 事件循环 │ 下载    │ 安全策略 │ 会话    │          │
│  └─────────┴─────────┴─────────┴─────────┴─────────┘          │
├──────────────────────────────────────────────────────────────┤
│ 平台层：字体候选路径 / 文件系统 / TLS / 时钟                  │
└──────────────────────────────────────────────────────────────┘
```

### 3.1 核心对象

| 对象 | 职责 |
| --- | --- |
| `LsmBrowserManager` | 进程级单例；持有窗口、标签页、Cookie Jar、历史与配置 |
| `Window` | 多窗口容器；同一窗口内多个 Tab；窗口级尺寸与前台标记 |
| `Tab` | 一个页面的全部状态：URL、DOM、CSSOM、存储、历史、滚动、表单值 |
| `Document` | `kuchikiki` DOM + 查询/修改封装 + 稳定选择器生成 |
| `NetworkStack` | 请求、重定向、Cookie、字符集解码、请求台账 |
| `JsRuntime` | JS 执行抽象；`BoaRuntime` / 未来 `V8Runtime` |
| `LayoutEngine` | 简化块级布局；产出文本行、元素矩形、页面尺寸 |
| `Renderer` | 软件光栅化；PNG 截图 |
| `StorageArea` | localStorage / sessionStorage / Cookie 全内存实现 |
| `Interaction` | 点击、输入、选择、按键、滚动、表单提交语义 |

## 四、分阶段路线图

### Phase 0 — 契约冻结与清理（本阶段）

- 梳理并冻结 `MCP_Web_LsmAgentBrowser_Use` 外部契约（actions / control_action / inspect info / 错误码）；
- 删除 Chrome/CDP 驱动、反检测注入、页面管控、浏览器进程治理等实现；
- 删除 Chrome/CDP 专用文档、环境变量、依赖；
- 建立新模块骨架与文档体系。

**验收**：工程中不存在 `chromiumoxide`、CDP 连接、Chrome 探测/启动代码；`cargo build` 通过。

### Phase 1 — 内核基础框架（本阶段）

- 内存窗口/标签页模型、page_id 生命周期；
- HTTP(S) 导航、重定向、Cookie、字符集、超时；
- HTML 解析、可变 DOM、CSS 选择器；
- 交互：点击链接/按钮、输入文本、选择下拉、按键、滚动、表单 GET/POST；
- 检查：title/url/text/html/elements/links/forms/cookies/storage/network/console/viewport；
- JS：Boa 执行 + 基于 DOM 快照的 `document` 兼容层；
- 截图：简化布局 + 软件渲染出 PNG；
- `sequence` / `batch` / `explore` / 错误码；
- 单元测试 + 集成测试（本地 mock HTTP 服务，不依赖外网）。

**验收**：`MCP_Web_LsmAgentBrowser_Use` 能对静态/服务端渲染站点完成「打开 → 读取 → 点击 → 输入 → 提交 → 截图」闭环。

### Phase 2 — 页面脚本与动态 DOM

> **第一步已实现**(2026-10):页面脚本执行管线(内联/外链)、每 Tab 持久 JS 线程(Boa)、
> 事件 listener 与 DOMContentLoaded/load、fetch/XHR 真实请求、虚拟时钟定时器、
> 动态 DOM 回写与布局重算。设计见 `05-阶段二页面脚本与动态DOM设计.md`。
>
> **第二步已实现**(2026-10):独立 CSS 级联引擎(特异性/`!important`/继承/单位/媒体查询/`var()`)、
> 外链样式表预取与原位内联、`getComputedStyle`/`matchMedia`、`inspect(stylesheets)`。
> 设计见 `06-阶段二第二步CSS级联与外链样式加载设计.md`。

- ✅ 执行页面内联/外链 `<script>`；事件循环（宏任务/微任务）——微任务经 run_jobs 驱动(已 resolve 续体可执行),宏任务为虚拟时钟定时器;
- ✅ DOM/BOM 绑定:往返同步模型(快照注入 + 动作回传 + 宿主应用到真实 DOM),`document`/`window`/`location`/`history`/定时器/`fetch`/XHR 可用;
- ✅ 表单事件(input/change/submit)、click 事件、preventDefault;冒泡经父链匹配实现,捕获阶段 ⏳;
- ✅ 动态节点变化后重建布局;
- ✅ CSS 级联与计算样式(选择器权重、`!important`、继承、单位、媒体查询子集、自定义属性);
- ✅ 外链样式表 `<link rel=stylesheet>` 宿主预取并原位内联,网络台账可观测;
- ⏳ 完整 CSS 层叠(动画/过渡/伪元素/`@supports` 精确求值)、`@import` 递归、图片/字体子资源——阶段三/四。

**验收**:依赖 JS 渲染的常见站点可读取正文与交互——**已达成**(`tests/web_use_engine.rs` 6 个端到端用例)。

### Phase 3 — 布局/渲染/窗口

> **第一步已实现**(2026-10):Flex 弹性布局 + relative/absolute/fixed/sticky 定位
> + z-index 绘制序。设计见 `08-阶段三布局引擎Flex与定位设计.md`。
>
> **第二步已实现**(2026-10):Grid 网格布局(显式/隐式网格、px/%/fr/auto/minmax/repeat
> 轨道、自动/显式放置、gap、条目/轨道对齐)。设计见 `09-阶段三第二步Grid网格布局设计.md`。
>
> **第三步已实现**(2026-10):图片子资源加载器(宿主预取 + 解码 + 全内存缓存)、
> 替换元素(`<img>`)排版、`object-fit` 渲染、`background-image` 基础绘制。
> 设计见 `10-阶段三第三步图片渲染与替换元素设计.md`。

- ~~Flex / Grid / 定位~~ ✅(08/09 号文档);~~图片渲染~~ ✅(10 号文档);文本排版(双向/emoji)待补;
- 原生窗口（winit 或平台原生）与多窗口/多标签 UI；
- 鼠标、键盘、滚轮事件投递；
- 页面缩放、滚动条、暗色模式；
- 截图完整页 ✅；**区域截图(clip)已实现**(2026-10，12 号文档)。

**验收**：headed 模式可人工观看与操作；截图与页面视觉一致。

### Phase 4 — 网络与存储完整化

> **基础框架已实现**(2026-10，11 号文档)：ResourceLoader、HttpCache、SecurityContext
> 三模块 + Cookie SameSite + CORS/CSP/混合内容代码。
>
> **第二步已实现**(2026-10，12 号文档)：HttpCache 接入主管线（主文档+子资源强缓存、
> from_cache 台账、cache_stats、LAEW_WEB_CACHE=off）；CSP 接入脚本管线（内联拦截+外链白名单
> +connect-src 门禁）；CORS 接入页面 fetch/XHR（跨域响应校验）。

- HTTP/2 完整、HTTP/3 可选、代理、连接池；
- Cookie 属性（Domain/Path/Secure/HttpOnly/SameSite）✅；第三方 Cookie 隔离 ⏳；
- localStorage/sessionStorage ✅；IndexedDB ⏳；
- 下载管理（进度、取消、重试）⏳；
- 缓存（强缓存/协商缓存，全内存 LRU）✅ 已接入主管线（协商缓存请求侧 ⏳）。

### Phase 5 — JS/Web API 扩展

- 切换到 V8 后端（`rusty_v8`）；
- WebSocket / SSE / fetch 流式响应；
- Canvas 2D、WebGL 可选、Web Audio 可选；
- Web Components / Shadow DOM / Observer 系列；
- Worker / Service Worker 可选。

### Phase 6 — 安全、调试与体验

- 同源策略、CORS、CSP 执行；
- 权限模型（摄像头/麦克风/定位/通知/剪贴板）；
- 进程/上下文隔离（可选多进程渲染）；
- 内置 DevTools（DOM/Console/Network/Storage）；
- 可选 CDP 服务端（仅作为对外调试接口，内核不依赖 CDP）。

## 五、兼容层设计原则

1. **外部契约不变**：工具名、action、参数名、响应信封、错误码由兼容层保证；内核重构不改变 Agent 提示词。
2. **参数宽容**：历史参数（`reuse_existing`、`connect_url`、`auto_relaunch`、`stealth`、`guard`、`overlay` 等）继续被接受；新引擎不适用的能力返回结构化说明，不报参数错误。
3. **错误可机械决策**：沿用 1xxx（参数）/ 2xxx（页面与动作）/ 3xxx（环境）/ 4xxx（人工介入）/ 6xxx（范围约束）分段。
4. **page_id 稳定**：open 获取；control/inspect 多轮复用；新标签页经 `spawned_page_id` 回传；close 释放。
5. **无浏览器进程**：`close(page_id="all")` 只清空内存中的窗口/标签页，不杀任何系统进程。

## 六、删除清单（Phase 0）

### 6.1 代码

- Chrome/CDP 驱动：`browser.rs` 旧实现、`browser_cdp.rs`、`browser_reuse.rs`、`browser_watchdog.rs`、`cdp_ext.rs`
- 反检测与注入：`stealth.rs`、`fingerprint.rs`、`browser_ns.rs`、`browser_spawn_arm.rs`、`browser_env_emulation.rs`
- 页面管控/状态栏/跟随/节奏/焦点/权限/噪声：`browser_overlay.rs`、`browser_status.rs`、`browser_follow.rs`、`browser_pace.rs`、`browser_focus.rs`、`browser_perm.rs`、`browser_noise.rs`
- 页面活性/台账：`page_liveness.rs`、`tab_ledger.rs`、`web_activity.rs`
- 旧工具层中直接使用 CDP 命令的模块（按新内核重写）
- 依赖：`chromiumoxide`
- 测试：`tests/web_use_smoke.rs`、`tests/stealth_reveal_smoke.rs`、`tests/browser_watchdog.rs`

### 6.2 环境变量

删除 Chrome/CDP 专用开关（浏览器模式除外）：`LAEW_CHROME_DEBUG_PORT`、`LAEW_WEB_STEALTH`、`LAEW_WEB_FINGERPRINT`、
`LAEW_WEB_SPAWN_ARM`、`LAEW_WEB_OVERLAY`、`LAEW_WEB_LOCKED_SCROLL`、`LAEW_WEB_HITL_SCOPE`、`LAEW_WEB_BLOCKER_PROBE`、
`LAEW_WEB_EVIDENCE`、`LAEW_WEB_PACE`、`LAEW_SITE_RISK_*`、`LAEW_WEB_RISK_COOLDOWN_SECS`、`LAEW_WEB_WIPE_*`、
`LAEW_WEB_FOCUS`、`LAEW_WEB_BACKGROUND_GATE`、`LAEW_WEB_HEADLESS_SOFTRENDER`、`LAEW_WEB_GEO` 等。

保留：`LAEW_BROWSER_MODE`（headed/hidden，阶段一 headed 自动回退无头）、`LAEW_WEB_MAX_TABS`、`LAEW_WEB_MAX_WINDOWS`、
`LAEW_WEB_EVAL_TIMEOUT_MS`、`LAEW_ALLOW_PRIVATE_ENDPOINT`、`LAEW_TLS_INSECURE`。

### 6.3 文档

- 删除 `docs/浏览器CDP工具/`；
- 删除 `docs/MCP_Web_Chrome_Use/` 中依赖 CDP/Chrome 的专项文档；
- 在 `docs/MCP_Web_LsmAgentBrowser_Use/` 重建 `MCP_Web_LsmAgentBrowser_Use` 契约与内核文档；
- 同步更新 `CLAUDE.md`、`AGENTS.md` 的架构、环境变量、文件树与文档地图。

## 七、风险与应对

| 风险 | 影响 | 应对 |
| --- | --- | --- |
| 阶段一不支持页面 JS 渲染 | 动态站正文可能为空 | 阶段二实现页面脚本；先以服务端渲染站点验收 |
| Boa 与 V8 行为差异 | 个别 JS 语法/API 不支持 | `JsRuntime` 抽象；V8 可选特性；语法错误返回结构化错误 |
| 简化布局与真实浏览器差异 | 截图与坐标不精确 | 阶段三补布局；自动化优先用选择器而非坐标 |
| 站点反爬识别 | 请求被拒 | 自研内核不使用伪装注入；以真实 UA 标识；多路径（表单/Cookie/重试） |
| CSS 子集覆盖不足 | 视觉与部分可见性判断偏差 | 建立站点回归集，按访问量补齐属性 |
| 大文件/图片内存占用 | 全内存架构压力 | 单资源大小上限 + LRU + 后台标签页回收 |

## 八、验证方式

- `cargo test --lib`：内核单元测试（URL 解析、Cookie、选择器、表单、编码、布局、渲染、JS 桥）；
- `cargo test --test web_use_engine`：本地 mock HTTP 站点端到端（open/inspect/click/input/sequence）；
- `./rebuild_restart_app.sh`：真实二进制构建；
- 人工冒烟：`./laew -p "打开 <本地或公网静态页> 并提取标题与正文"`。
