# 39 · 安全验证页 open 阻断直出与 WebGL 提示归一

> 日期：2026-10-29
> 方案：`tmpPlan/20261029_142451_zhipin安全验证页open阻断引导与能力提示修复.md`
> 场景：BOSS 直聘采集任务入口重定向到 GeeTest 安全验证页；`open` 已返回验证页标题与 URL，却仍给出搜索引导，Agent 必须额外 `explore(blockers)` 才能确认阻断。

## 一、真机证据与边界

- release `0d123e1`，`laew --debug -f`，`LAEW_BROWSER_MODE=headed`。
- `https://www.zhipin.com/` 最终落到 `/web/passport/zp/verify.html`，标题「安全验证 - BOSS直聘」；页面为 GeeTest 雷达按钮 ready 形态（文档 37 的形态 A），未出现 `reset.php` 自愈循环。
- `open` 返回固定「输入关键词 → 搜索 → 等待结果」引导；随后一次 `explore(text/elements/page_meta/blockers)` 才得到 `blocked=true`、`suggested_action=request_human`。
- 页面需要人工安全验证，无法继续岗位采集，按停测要求终止任务。现有证据不能判定验证页由 IP、会话或哪种指纹信号触发；不把它归因成某个已证实的内核缺陷，也不通过改 UA 或反复刷新尝试绕过。

## 二、open 直接复用 blockers 判定

`tools/mcp_web_lsm_browser_use/mod.rs::run_open` 在页面建立后调用内核既有的 `browser::inspect::inspect(info=blockers)`，将同一份报告附到 `open.data.blockers`。不新增第二套关键词或站点特判。

`open_next_steps` 现在按报告生成引导：

- 普通页面：保留原搜索示例，并补实际 `page_id`；
- 阻断页面：首步为 `control(request_human)`，携带实际 `page_id` 与合法原因（`captcha` / `login` / `manual_verify`）；第二步为 `inspect(blockers)` 复查。

这只是**可执行建议**：不自动弹窗、不机械阻断 Agent，也不改变 Plan 阶段「只规划、不提前打扰人工」的语义。实际人工介入仍由 Agent 根据任务阶段决定。`page_id` 必须放在 control 顶层，`request_human` 才能关联页面 URL、切换该页控制权并执行可见性守卫。

## 三、WebGL 能力提示改为中性语义

验证/指纹脚本也会申请 WebGL，不是地图站独有。`open.capability_gaps` 现统一表述为：

- 内核无 WebGL，对应画布区域不会绘制像素；地图底图、三维场景或验证题面都可能受影响；
- **申请过 WebGL 不等于主画面依赖 WebGL**；仅当任务依赖该画布像素时才建议 Chrome，否则仍可读取 DOM。

与文档 30 的 `page_health` 优先级和中性措辞对齐，避免在验证码页输出「需要像素级地图底图」的误导提示。

## 四、回归

`tests/web_use_engine.rs` 新增 mock 安全验证页（含一次 WebGL 申请）：

- `open.data.blockers.blocked=true`，首条阻断类型为 `captcha`；
- 首个 next step 为 `request_human`，原因与真实 `page_id` 正确；第二步复查 blockers；
- WebGL 提示包含验证题面边界，不再把 hint 写成地图底图专用；
- 普通首页仍返回 `blocked=false` 与原 `input_text` 搜索引导。

结果：

- `web_use_engine`：26/26；
- `cargo test --lib`：2650 passed，0 failed，7 ignored；
- 全量 `cargo test` 仅命中既有 macOS AX 环境失败 `macos_inspect_reports_not_authorized_not_unavailable`（pid=1 未暴露 AX 窗口）；跳过该已知基线用例后，其余单元与集成套件全部通过；
- 真站未复测，不宣称岗位采集任务已完成，也不宣称安全验证可以通过内核改动消除。
