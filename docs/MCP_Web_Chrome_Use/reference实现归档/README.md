# 历史 MCP_Web_Chrome_Use(Chrome/CDP)实现归档

本目录完整归档了 **2026-10-20 提交 `49d1891f`(`feat(browser): 以 LsmAgentBrowser 进程内内核替换 Chrome/CDP 实现`)中删除的 Chrome/CDP 实现源码**,共 47 个文件,取自其父提交 `b459b37`,**逐字保留、零逻辑改写**;仅将内容中的工具名 `MCP_Web_LsmAgentBrowser_Use` 机械替换为 `MCP_Web_Chrome_Use`(该实现即历史 MCP_Web_LsmAgentBrowser_Use 的 Chrome/CDP 方案本体)。

> **性质:参考实现归档,不参与编译。** 其中引用的模块路径(如 `crate::agent::browser`、`src/agent/tools/mcp_web_use/`、`src/config/site_risk` 等)是当时仓库的布局,与当前代码库不完全一致,直接搬回 `src/` 无法编译。当前生效的可编译实现见:
>
> - `src/agent/chrome_browser/` — Chrome 子进程驱动(基础框架,分阶段补齐本归档能力)
> - `src/agent/tools/mcp_chrome_use/` — `MCP_Web_Chrome_Use` 工具契约层
> - 总体设计与分阶段移植计划:`docs/双浏览器工具架构/01-设计与解决方案.md`

## 归档清单(按原路径)

| 原路径 | 内容 | 移植状态 |
|--------|------|----------|
| `src/agent/browser.rs` (3937 行) | Chrome/CDP 浏览器总管:启动参数、反检测注入、页面生命周期 | 基础框架已重写,高级能力待移植 |
| `src/agent/browser_cdp.rs` / `cdp_ext.rs` | CDP 连接与扩展协议封装 | 已由 `chrome_browser` 基础框架覆盖 |
| `src/agent/browser_env_emulation.rs` / `browser_viewport.rs` | 环境与视口仿真 | 待移植 |
| `src/agent/browser_focus.rs` / `browser_noise.rs` / `browser_pace.rs` | 焦点/噪声/节奏拟人化 | 待移植 |
| `src/agent/browser_follow.rs` / `browser_reuse.rs` / `browser_perm.rs` / `browser_ns.rs` | 会话复用、权限、命名空间 | 待移植 |
| `src/agent/browser_overlay.rs` | headed 可视化蒙层与人工操作拦截 | 待移植(配合 HITL) |
| `src/agent/browser_spawn_arm.rs` / `browser_watchdog.rs` / `browser_window.rs` / `browser_status.rs` | 子进程拉起、看门狗、窗口管理、状态栏 | 基础框架已覆盖拉起/回收,看门狗待移植 |
| `src/agent/browser_mode.rs` | headed/hidden 模式判定 | 基础框架已覆盖(`LAEW_CHROME_MODE`) |
| `src/agent/stealth.rs` (424 行) | 反检测隐身注入 | 待移植 |
| `src/agent/fingerprint.rs` (981 行) | 浏览器指纹一致化 | 待移植 |
| `src/agent/page_liveness.rs` / `web_activity.rs` / `tab_ledger.rs` | 页面活性/活动台账/标签账本 | 待移植 |
| `src/agent/display_topology.rs` | 显示器拓扑真值(该文件后被重新实现,见 `src/agent/display_topology.rs`) | 已在当前库存在 |
| `src/agent/tools/mcp_web_use/*.rs` (17 个文件,含 `control/mod.rs` 1725 行) | Chrome 版工具动作全集:表单/输入/跟随/守卫/HITL 管控/登录墙/页面状态/解锁区/抹除观察等 | 基础框架先实现 open/navigate/click/type/eval_js/screenshot,其余按设计文档分期移植 |
| `src/config/site_risk.rs` (633 行) | 站点风险熔断配置 | 待移植 |
| `tests/browser_watchdog.rs` / `stealth_reveal_smoke.rs` / `web_use_smoke.rs` | Chrome 时代测试 | 待移植 |
| `examples/stealth_baseline_probe.rs` | 反检测基线探针 | 待移植 |
| `docs/MCP_Web_Chrome_Use/*.md`(38 篇)→ 已恢复为 `docs/MCP_Web_Chrome_Use/` | Chrome 版知识库 | 已恢复 |
| `docs/浏览器CDP工具/*.md`(1 篇)→ 已恢复为 `docs/MCP_Web_Chrome_Use_CDP技术方案/` | CDP 技术方案 | 已恢复 |
