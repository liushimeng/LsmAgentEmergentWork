# 17 - headed 视口铺满根治与主文档健康诊断(第 161 轮)

> 状态:**现行**。窗口/视口几何的更早轮次背景见 `14-窗口铺满置前与弃权如实转达.md`(第 156/158 轮)
> 与 `16-内网站点实测缺陷修复-首启空标签页-空白页诊断-句柄自愈.md`(第 159 轮);
> 本文只描述第 161 轮的两项根治,冲突时以本文为准。

## 0. 实测事故(2026-10-12 16:00,内网站点运营后台)

用户报告:**「弹出的 Chrome 显示为白屏,显示区域没有铺满 Chrome 浏览器」**。

`llaew_20261012_160035.log` 里 open 响应的铁证:

```json
"headed_window": {
  "fill": true, "adjusted": false,
  "measured": {
    "outer":   {"width": 1920, "height": 1080},
    "inner":   {"width": 800,  "height": 600},
    "chrome_height_raw": 480, "chrome_height_fallback": true
  }
},
"viewport": {"viewport": {"width": 800, "height": 600}, "expanded": false}
```

窗口外框 1920×1080(`--window-size` 已生效),但**页面视口恒 800×600** ——
内容只渲染左上角 800×600,右侧/下方大片空白(Chrome 对视口外区域渲染为白色)。
叠加该站 WAF 对主文档返回 HTTP 400 空 HTML,人工看到的就是「白屏 + 没铺满」。

## 1. 根因 A:chromiumoxide 默认 viewport=Some(800×600) 钉死三模式页面

- `chromiumoxide 0.9.1` `BrowserConfig` 默认 `viewport: Some(Viewport::default())`,
  `Viewport::default()` = **800×600**(`browser/config.rs:154` / `handler/viewport.rs`);
- 每个 page target 初始化都会按该配置发 `Emulation.setDeviceMetricsOverride(800,600)`
  (`handler/target.rs:387` → `handler/emulation.rs::init_commands`);
- `hide()`(**老式** headless)会在自身内部清掉默认视口(`config.rs:331`),但本项目
  三档用的是 `with_head()` / `new_headless_mode()`,**都不清** → headed / hidden /
  new_headless 的每个页面全被钉 800×600;
- 连带纠正一个历史误判:第 156 轮记录「macOS 上 inner 在窗口布局实现前恒为默认
  800×600」并为此引入 `plausible_chrome_height` 兜底 —— 真相不是时序,是视口覆盖
  钉死(Windows 同样中招,只是当时仅在 macOS 观测到);`chrome_height_raw=480`
  失真同源。`plausible_chrome_height` 兜底保留(防真实场景的 inner 迟滞),但其
  「布局时序」注释语义已过时;
- hidden 模式此前未暴露:页面内容超视口时 `fit_viewport_to_content` 自动扩 2K 把
  钉死掩盖了;空内容页面(如 WAF 400 空文档)不触发扩展 → 钉死在 800×600。

### 修复(`src/agent/browser.rs::ensure_browser`)

窗口尺寸设定之后按模式显式配置 viewport:

| 模式 | 配置 | 语义 |
| ---- | ---- | ---- |
| **Headed**(launch) | `viewport(None)` | 禁用视口仿真(Playwright `viewport:null` 同款),页面视口 = 窗口内容区(1920×1080 外框 → 视口 ≈1920×940),天然铺满;人工拖动窗口视口自适应 |
| **Hidden / NewHeadless** | `viewport(Viewport{width:win_w, height:win_h})` | headless 窗口即视口,显式 1920×1080,不再依赖「内容超视口才扩展」兜底 |
| **connect**(接管用户浏览器) | 不变 | `Browser::connect` 走 `HandlerConfig::default()`(viewport 本就 None),从不覆盖用户页面 |

顺序铁律不变:`fit_headed_window` 仍**早于** `fit_viewport_to_content`
(`setWindowBounds` 会清除 device metrics 覆盖);与第 158 轮「保持当前窗口尺寸
不铺满屏幕」语义正交(那管窗口外框,这管页面视口)。

### 实测验证(2026-10-12,本机 Mac Studio)

- headed(默认窗口):视口 **1920×993**(外框 1920×1080,chrome 高 87)—— 修复前 800×600;
- hidden:视口恒 **1920×1080** —— 修复前 800×600;
- 内网站点登录页(headed):视口 1920×993,页面渲染区铺满窗口。

## 2. 根因 B:open 响应不透出主文档健康度,Agent 盲试 5 轮

第 159 轮已让 Network 证据(含 `loadingFailed`)进每页环形缓冲,但 Agent 要自己
想到 `inspect(network)` 才看得到主文档 400。本次日志里 SubAgent-Work 连续
reload/wait/fetch 探了 5 轮(16:02:29 → 16:05:29,约 3 分钟)才确认不可达 ——
open 返回时主文档状态码明明已经在缓冲里。

### 修复(`src/agent/tools/mcp_web_use/mod.rs::page_health_field`)

open(新建 + 同 URL/origin 复用两条路径)导航完成后聚合诊断,**fail-open、不命中
不带字段(零噪声)**:

- 主文档状态:扫 Network 环形缓冲 `phase=response` 且 `url==final_url` 的条目
  (倒序取最后一条,天然覆盖重定向/多次 reload);
- 空文档判定(纯结构证据,不做关键词猜测):排除 `[data-laew-agent]` 注入层
  (高亮框/徽标/蒙层/状态条)后 DOM 元素数 ≤3(html/head/body)即无真实内容。
  **不**叠加 body 文本门槛 —— 注入层文案(「LAEW Agent 控制中」等约 50 字)恒计入
  `innerText`,叠加会把真空页误判为非空(内网站点 400 空文档实测如此);
- 只读 evaluate、不改 DOM,connect 模式同样零注入。

命中即附:

```json
"page_health": {
  "blank": true,
  "reason": "main_doc_error",          // 或 "empty_dom"
  "main_doc_status": 400,
  "note": "主文档返回非成功状态(服务器/WAF 拒绝;瑞数类反爬对自动化浏览器恒 4xx 空 HTML),reload 通常无效",
  "next_action": "inspect(network) 查看全部拦截证据;改用 open(reuse_existing=true) 复用已登录浏览器(真实指纹+登录态);仍不可达时 request_human 请人工打开,或如实报告不可达并停止"
}
```

两种 reason 的分工:

| reason | 判据 | 文案基调 |
| ------ | ---- | -------- |
| `main_doc_error` | 主文档响应状态 ≥400 | 硬结论:服务器/WAF 拒绝,reload 无效,直接改道 |
| `empty_dom` | 状态 2xx/未知但 DOM 无真实内容 | 软结论:可能 SPA 首帧未跑完,先 wait 1~2s 复查,持续空白再按反爬处理 |

SPA 首帧误报由「软文案 + 先复查」消化,不会把正常加载页面错误定性为被拒。

### 实测验证

内网站点登录页 open 一轮即返回:

```json
"page_health": {"blank": true, "reason": "main_doc_error", "main_doc_status": 400, ...}
```

模型第一轮就能拿到「WAF 拒绝 + 处置建议」,按 `next_action` 改道
`reuse_existing=true`(复用用户已登录 Chrome,真实指纹)或 `request_human`,
不再空烧 3 分钟 reload。

## 3. 关于 WAF 400 本身

主文档恒 400 空 HTML 是服务器侧反爬决策(瑞数形态 cookie `zNqSbRHht4usO/P`
已种上仍拒),**不是客户端可根治的缺陷** —— 第 159 轮启动参数加固
(剔除 `--enable-automation` + `--disable-blink-features=AutomationControlled`)后
依然如此。laew 的定位是把「被拒」从隐性失败变成首轮可见的结构化信号(本文 §2),
处置权交给模型(reuse_existing / request_human / 如实报告)。网页取证纪律
(`04-网页取证纪律.md`)继续生效:禁止改用 curl 探测任务目标站点。

## 4. 回归锁

`src/agent/tools/mcp_web_use/tests.rs`(均为 `#[ignore]` 真浏览器用例,本地
`cargo test --lib mcp_web_use -- --ignored --nocapture`):

1. `real_browser_viewport_fills_window`:headed 视口 ≥1600×800 且外框 ≥1900(默认
   1920×1080);shutdown 后 hidden 相位视口恒 1920×1080(±2);
2. `real_browser_site_viewport_and_page_health`:真站点,视口铺满断言无条件;
   page_health 自洽断言条件式 —— DOM 无真实内容 ⇒ 必须报
   `reason ∈ {main_doc_error, empty_dom}`(且 main_doc_error 必带 ≥400 状态码),
   有真实内容 ⇒ 不得误报(服务器未来放行 200 时测试依然有效)。
