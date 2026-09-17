# Chromium-WebUse Agent（第 11 角色）设计与解决方案

> 2026-09-16 第 61 轮创建。前置调研：`Rust语言操作浏览器CDP工具相关的技术信息.md` /
> `Rust语言操作内存浏览器.md` / `Rust语言操作已经打开的浏览器.md`（选型与 CDP 机制），
> 以及 Go 工程 `go-web-debug-tool`（MCP 风格浏览器操控服务，35 action + 17 info）的参数级移植参考。
>
> **2026-09-17 第 75 轮更新**：修复 delegate_to 路由错配（网页任务被错误路由到 WindowUse），
> 详见 §0「第 75 轮 P0 修复摘要」。

## 0. 第 75 轮 P0 修复摘要(2026-09-17)

### 0.1 问题背景

实测「打开网址 wenxin.baidu.com,中间有一个对话的输入框,输入内容..., 找到搜素确认的按钮,
点击按钮」任务 51 次 LLM 调用、299 秒、409k cache_read,最终以 `[windowuse-mode] Bash`
权限拒绝 + 16 轮 0 工具调用失败(Debug 报告:`DebugReport/debug_report_20260917_114355_ade10f.md`,
P0-1 + P0-2)。

### 0.2 根因

`src/agent/main_work/delegate.rs:142-166` 的 `infer_delegate_to` 把第 67 轮为修微信
任务加的「输入框」「点击按钮」「鼠标」「滚轮」等中文 GUI 词视为强 GUI 证据,但这些
词在 HTML Web 场景同样存在。命中后压制 web_hit → 误判 `WindowUse` → WindowUseRunner
无 Browser* 工具 → 死循环。

### 0.3 修复(本轮 4 处代码改动)

1. **`src/agent/main_work/delegate.rs`** — 拆分 GUI 关键词为 `WINDOW_USE_STRICT_KEYWORDS`
   (桌面应用专属)与 `WEB_DOM_GUI_KEYWORDS`(Web 通用,不再作为 GUI 强证据);`infer_delegate_to`
   优先级改为:桌面 GUI 强信号 → WindowUse;web 命中 → WebUse(shell 不再压制);仅 shell → SubAgent。

2. **`src/agent/main_work/mod.rs:248-252`** — Main-Work JSON 解析失败时构造的兜底
   WorkFlowPlan 也会跑一次 `infer_delegate_to_for_plan`,确保 fallback 路径也能纠正。

3. **`src/agent/extrace.rs` + `src/agent/subagent.rs` + `src/agent/web_use.rs` +
   `src/agent/window_use.rs`** — `ExecutionTrace` 新增 `runner_role`/`intended_role`
   字段,Runner 在 `run_unit_inner` 入口注入;`collect_failure_signals` 计算
   `delegate_mismatch:runner=X intended=Y` 弱信号(`runner_role != intended_role && tool_calls == 0`)。

4. **`src/tui/dispatch.rs:461-475` + `src/agent/quality.rs`** — TUI 任务失败块新增
   `[路由错配]` 诊断行;QC WebUse/WindowUse early-terminate 文案加
   `delegate_mismatch` 分支,给出明确文案。

### 0.4 兼容性

- 新增 `ExecutionTrace.runner_role`/`intended_role` 字段加 `#[serde(default)]`,旧
  trace 反序列化无破坏。
- `delegate_mismatch` 是**弱信号**,不进入 `is_failed()`,仅作诊断提示。
- 全部 7 条旧 delegate 推断测试保持 PASS(`wechat_powershell_steps_route_to_windowuse`
  中"通讯录"仍在 strict 列表)。新增 5 条回归测试(详见 `src/agent/main_work/delegate.rs::infer_tests`)。
- 回滚:单 commit revert 即可,无 DB 迁移。

## 1. 需求与定位

新增 Agent **`LsmAgentEmergentWork-Chromium-WebUse`**（角色枚举 `AgentRole::WebUse`，serde 名 `webuse`），
职责：**一切涉及网页的操作**——网页浏览、信息收集、爬虫、登录 Web 页面后的页面操作、
点击/输入/滚动/截图/DOM 提取/Console·Network 观察等。Agent 集群中任何环节需要网页操作，
一律委派给本角色执行（Yolo 分类 → Main-Work 拆 WorkFlow `delegate_to="webuse"` → WebUseRunner 执行）。

核心要求映射：

| 需求 | 设计 |
| ---- | ---- |
| 优先内存中的 Chrome、优先无头 | 默认 `--headless=new` 拉起独立 Chrome 进程 + 一次性 user-data-dir |
| 自动检测浏览器 | `browser/detect.rs` 跨平台候选路径 + PATH 查找：Chrome → Edge → Chromium/Brave；**Firefox/Safari 不支持 CDP，仅检测提示不接入** |
| Windows/macOS/Ubuntu/Linux | chromiumoxide 纯 Rust + rustls，平台差异封闭在 detect 层 |
| 兼容未安装浏览器 | `BrowserNew` 返回结构化错误码 `3001` + 安装引导文案，不 panic、不崩溃流程 |
| page_id 管理与反复调用 | 进程内 `BrowserManager` 单例：page_id 不透明字符串注册表，多轮对话跨 WorkFlow 复用 |
| 点击链接/新开页面返回新 page_id | 动作前后 diff + `Target` 事件 adopt，响应携带 `spawned_page_id` |
| 打开/关闭内存浏览器 | `BrowserNew`（首个页面拉起浏览器）/ `BrowserClose`（引用计数，最后页面关闭才杀进程） |

## 2. 技术选型

**chromiumoxide 0.9**（+ futures 0.3）。理由：

- 单 crate 覆盖 `Browser::launch`（内存无头）与 `Browser::connect`（接管已开浏览器）双模式，
  与本目录三篇前置文档的主线结论一致；
- 强类型 CDP 绑定，Page/Element 语义与 go-web-debug-tool 的 chromedp 最接近，移植成本最低；
- tokio 原生，与 laew 运行时一致；rustls 链条不引入系统依赖。

已知代价：编译期代码生成较大，首次全量编译时间增加（可接受，release 增量编译后无感）。

## 3. 架构

```
src/agent/browser/            CDP 驱动层（平台/协议差异封闭）
  mod.rs        BrowserManager 单例(OnceCell+tokio::Mutex)：launch/connect、page 注册表、
                PageEntry{page, console 环形缓冲, network 环形缓冲, created_at}、
                spawned_page adopt、引用计数关闭、结构化错误码
  detect.rs     浏览器二进制检测：平台候选路径 + PATH fallback + 版本探测
src/agent/tools/browser.rs    工具层薄封装（5 个 Tool，镜像 tools/window.rs 模式）
src/agent/web_use.rs          WebUseRunner（镜像 WindowUseRunner 骨架）
```

### 3.1 工具面（写/读对偶，移植 go-web-debug-tool 的收敛设计）

| 工具 | 参数 | 说明 |
| ---- | ---- | ---- |
| `BrowserNew` | `url`★, `headless`, `wait_until`(load/domcontentloaded/networkidle), `user_agent`, `block_resources[]`, `connect_url` | 新建内存浏览器页面；`connect_url` 存在时接管已开浏览器（需对方带 `--remote-debugging-port`）。返回 `{page_id,title,final_url}` |
| `BrowserList` | — | 列出存活页面 `[{page_id,url,title,created_at}]`，返回前做存活性清理 |
| `BrowserClose` | `page_id`★ | 关闭页面；幂等（重复关闭返回 2000 语义文案）。最后一个页面关闭时回收浏览器进程 |
| `BrowserControl` | `page_id`★, `action`★(enum), `params`{扁平可选字段} | 全部写操作单一入口（见 §3.2） |
| `BrowserInspect` | `page_id`★, `info`★(enum), `params`{扁平可选字段} | 全部只读观察单一入口（见 §3.3） |

### 3.2 BrowserControl action 清单（移植 go 版 35 action 的核心子集 + 拟人化）

`click` / `human_click`（视口校验+遮挡检测+自动 scrollIntoView）/ `right_click` / `double_click` /
`hover` / `scroll`（selector | use_wheel | window.scrollBy 三模式）/ `scroll_to` /
`key_press`（特殊键+修饰键）/ `press_sequence` / `input_text`（SendKeys→JS 原生 setter 双路径回落，
React 受控组件兼容）/ `human_input`（逐字符随机延迟）/ `clear_input` / `upload_file`（绝对路径数组）/
`select_option` / `new_tab` / `close_tab` / `navigate` / `back` / `forward` / `reload` /
`wait`（selector visible/hidden/attached 或 duration_ms）/ `eval_js`（expression/expression_b64 双通道，
returnByValue 严格还原）/ `set_cookie` / `delete_cookie` / `set_storage` / `clear_storage` /
`set_viewport` / `screenshot`（save_path 落盘与 base64 互斥，**禁止大 base64 进 tool_result**）/
`heartbeat`。

元素定位统一 **CSS selector + nth**（与 go 版一致）；坐标只是派生手段。

### 3.3 BrowserInspect info 清单

`console` / `network`（每页 500 条环形缓冲 + `collection_healthy`/`last_event_at` 健康度）/
`elements` / `dom`（max_depth 硬上限 10 + node_count_limit 默认 2000/硬上限 5000 + truncated 标记）/
`localstorage` / `sessionstorage` / `cookies` / `screenshot`（含节点截图、save_path）/
`page_meta` / `viewport` / `url` / `title` / `ping`（永远成功信封 + ok 字段）。

### 3.4 统一信封与错误码（移植 go 版，Agent 可机械决策）

工具返回文本统一为 JSON：`{"code":int,"message":str,"data":{...}}`。

| code | 语义 | Agent 对策 |
| ---- | ---- | ---- |
| 0 | 成功 | — |
| 1001 | 参数错误 | 修正参数重试 |
| 2000 | page_id 不存在 | 重新 BrowserList 同步索引 |
| 2001 | CDP 断连 | 重连/重建页面 |
| 2002 | 动作执行失败（超时/selector 未命中） | 换 selector 或换 use_js 路径 |
| 2003 | 页面崩溃 | 重建页面 |
| 3000 | 内部错误 | 上报 |
| 3001 | 未检测到可用浏览器 | 输出安装引导（Chrome/Edge/Chromium），任务降级 |

### 3.5 浏览器会话管理

- **page_id**：`"p_" + rand 4 字节 hex`（8 位后缀），服务端生成，Agent 视为不透明字符串；page_id 即会话标识。
- **启动**：首个 `BrowserNew` 拉起 Chrome（`--headless=new` 默认；`--remote-debugging-port=0` 自动端口；
  tempfile 一次性 user-data-dir）；`connect_url` 模式 `Browser::connect` 接管（**connect 模式 close 不杀用户浏览器**）。
- **共享浏览器进程**：同一 BrowserManager 只持有一个 Browser 实例，新页面 `browser.new_page()` 开 Tab。
- **派生页面 adopt**：订阅 Target 事件 + 动作前后 `get_targets()` diff，未登记 page target 自动接管，
  经响应 `spawned_page_id` 回传。
- **关闭**：`BrowserClose` 移除 entry；页面计数归零时 `browser.close()` 杀进程（launch 模式）。
- **并发**：BrowserManager 全局 `tokio::Mutex`，同页动作天然串行。

## 4. Agent 集群接入（镜像 WindowUse 链路）

1. `context.rs` — `AgentRole::WebUse`（`#[serde(rename = "webuse")]`），`as_str()`/`From<&str>` 加分支。
2. `profile.rs` — `WEB_USE_AGENT_NAME = "LsmAgentEmergentWork-Chromium-WebUse"`，`web_use_profile()`
   （工具集 = Read + 5 浏览器工具）；测试计数 10→11。
3. `system_prompt/mod.rs` — `WEB_USE_BASE_PROMPT`（职责/作业规范/错误码对策表/无浏览器降级话术）+
   `web_use_tools_hint()` + 双协议 tail + `SystemPrompt::web_use()`；注册表与工具对齐测试同步加。
4. `tools/mod.rs` — `pub mod browser;` + `web_use_registry()`。
5. `web_use.rs` — `WebUseRunner` 镜像 `WindowUseRunner`：`run_unit`/`run_unit_with_cancel`/`run_unit_inner`，
   trace/取消/Agent-Memory 全链路复用；失败兜底（0 工具调用且无动作关键词 → failed）。
6. `orchestrator.rs` — `run_wf_unit` if/else 改 `match` 三分支；字段 + 构造 + 两个调用点传参。
7. `main_work.rs` — `lenient_delegate_to` 加别名（`"webuse"|"chromium"|"browser"|"web"|"浏览器"|"网页"`）；
   新增 `WEB_USE_KEYWORDS`（浏览器/网页/网站/打开网址/登录网站/爬虫/抓取页面/chrome/截图网页…），
   `infer_delegate_to` 加 WebUse 分支；运行时 prompt 改「三选一」；强制覆盖逻辑加 `"webuse"`。
8. `yolo.rs` — 关键词表 + `infer_suggested_delegate` 返回 `"webuse"`；YOLO_BASE_PROMPT 追加
   「网页/浏览器操控类任务最低按 medium 档分类」规则。
9. `config/agent_memory.rs` / `config/session_memory.rs` — 角色字符串 fail-closed match 加 `"webuse"`
   （**漏改会导致旧数据查询报错**）。
10. `quality.rs` — `unit_label` 改 match 加 WebUse 文案。
11. `agent_message.rs` — 显示名 match 加分支。
12. QC / SessionContext / Debug / 取消 / 并行：Runner 产物 `SubFlowOutcome`+`ExecutionTrace` 天然复用，零改动。

## 5. 测试与验证

- 单元测试：角色字符串往返、profile 计数、delegate_to 别名解析、关键词推断、detect 候选路径构造、
  错误码信封序列化、环形缓冲截断、DOM 树闸门。
- e2e（`testReport/run_e2e.sh` 新增一节）：mock LLM 下 `delegate_to="webuse"` 路由断言；
  无浏览器环境 `BrowserNew` 返回 3001 引导文案断言（CI 无 Chrome 也可跑）。
- 真实验证（本机有 Chrome）：`./laew -p "用浏览器打开 example.com 并截图到 /tmp/laew_web.png"`。
- 构建：`./rebuild_restart_app.sh`。

## 6. 风险与降级

| 风险 | 对策 |
| ---- | ---- |
| chromiumoxide 编译慢 | 一次性成本；Cargo.lock 入库后增量编译无感 |
| 目标站点 bot 检测 | headless=new + UA 覆盖已可过大部分场景；反检测加强（zendriver/注入脚本）列为后续迭代 |
| 无浏览器环境 | 3001 结构化降级，Yolo 失败回流建议用户安装或改用其它方案 |
| 大截图/DOM 撑爆上下文 | save_path 落盘优先；DOM 双闸门 + truncated 标记；工具结果接入既有截断体系 |
| 僵尸 Chrome 进程 | tempfile user-data-dir 自动清理 + 页面归零关进程 + 进程退出时 Drop 兜底 |
