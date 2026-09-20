# AGENTS.md — LsmAgentEmergentWork

供 AI Agent Tools（Claude Code / Codex / Hermes / OpenCode / pi / OpenClaw 等）自动加载的工程入口说明。

## 工程是什么

由 LLM 驱动的 Rust Agent CLI（二进制名 **`laew`**）。支持 Anthropic（anthropic-messages）与
OpenAI（openai-completions）双协议，**多 Agent 架构**（6 角色 + 三档难度），
内置 Bash / Read / Write 三个工具，TUI 多轮对话 + `-p` 单轮模式 + `-f` 文件提示词模式。
TUI 支持斜杠命令自动补全（Tab 补全 + 行内提示）和 @ 文件提及（`@路径` / `@"带空格"` / `@路径#L10-20` 行区间，输入 @ 后 Tab 实时路径补全、目录可钻取；命中文件内容以 `<<<LAEW:ATTACHMENTS>>>` 附件块自动注入上下文，实现见 `src/agent/attachments.rs` + `src/tui/mention.rs`）。
配置持久化在 **根目录** SQLite（`LsmAgentEmergentWork.db`），不使用配置文件。
完整架构设计见 `docs/多Agent架构重构/01-设计与解决方案.md`。

## 常用命令

```bash
./rebuild_restart_app.sh      # cargo build --release → 拷贝 ./laew 到根目录(改代码后必跑);支持 --debug
cargo build                  # 快速编译检查
cargo test                   # 单元测试
bash testReport/run_e2e.sh   # 端到端(mock LLM,无需真实 Key;含 TUI 子屏 tmux 自动化用例)
./laew --version             # 版本 + 编译时间 + git hash
./laew --help                # CLI 指南
./laew                       # TUI 交互模式
./laew -p "任务描述"          # 单轮任务模式
./laew -f /path/to/prompt.md # 从文件读取提示词执行(支持绝对/相对路径)
./laew -debug [-p "任务" | -f prompt.md]  # 调试模式(等价 --debug):采集各 Agent 输入输出/性能/质量,任务后由 Debug Agent 评估并生成报告;同时在**工作目录**输出 DEBUG 级运行日志文件
./laew --info [-p "任务" | -f prompt.md]  # 输出 INFO 级运行日志文件(不生成 Debug 报告);与 --debug 均支持单横线(-info)与大小写变体(--INFO/-DEBUG)
./laew --resume [N|id]                    # 恢复历史会话后进入 TUI(第 95 轮;短参 -c,无参=最近一次;首条输入前生效)
./laew --sessions                         # 列出持久化历史会话后退出(不进 TUI)
./laew provider add|list|use|delete ...
```

注意：crates.io 在本机网络较慢，已在 `~/.cargo/config.toml` 配置 rsproxy.cn 镜像。

## 环境变量

| 变量 | 取值 | 行为 |
| ---- | ---- | ---- |
| `LAEW_TLS_INSECURE` | `1`/`true`/`yes`/`on` | TLS 全局宽松：所有 endpoint 跳过证书校验（仅调试） |
| | `0`/`false`/`no`/`off` | TLS 全局严格：所有 endpoint 严格校验（安全基线） |
| | 未设置（默认） | 自动模式：endpoint 主机为 IP（IPv4/IPv6）时自动跳过证书校验，域名主机仍严格校验。适配 IP + 自签名证书的内网/自建 HTTPS 网关；仅跳过校验，TLS 加密不降级；rustls 纯 Rust 实现，Windows/macOS/CentOS/Ubuntu 行为一致。设计见 `docs/自签名证书TLS适配/01-设计与解决方案.md` |
| `LAEW_ALLOW_PRIVATE_ENDPOINT` | `1` | SSRF 防护放行私网/loopback endpoint（本地 Ollama / 局域网 / mock 测试 provider 用；默认拦截，见 `src/agent/safety/url_safety.rs`） |
| `LAEW_BASH_UTF8` | `1`/`true`/`yes`/`on` | Bash 工具为子进程注入 UTF-8 环境（`PYTHONUTF8=1`/`PYTHONIOENCODING=utf-8`/`LC_ALL=C.UTF-8`），消除 Windows 区域设置(GBK)导致的 python/coreutils 输出乱码；默认关闭。行尾(CRLF)不受影响，精确 diff 场景脚本仍需 `reconfigure(newline=...)`，见 `src/agent/tools/bash.rs`（2026-09-13 第 50 轮新增） |
| `LAEW_LOG_CLIP` | 正整数（默认 `4000`） | `--debug`/`--info` 运行日志文件中单字段（LLM 思考文本/工具参数/结果等）的截断长度（字符数）；`0`/非法值回退默认。见 `src/logging.rs`（2026-09-17 第 69 轮新增） |
| `LAEW_AUDIT` | `off`/`0`/`false`/`no` | 关闭决策审计写入（默认开启）。开启时 5 个决策点（Yolo 分类 / Plan 规划 / Main-Work 拆解 / QC 判定 / Compact 压缩）各追加一条结构化 JSON 行到根目录 `AuditTrail/audit_{session_id}.jsonl`（已 gitignore），记录「输入上下文→决策结论→决策依据」三段式 + 耗时/token/扩展字段，全字段脱敏截断，fail-open 不影响主流程。见 `src/agent/decision_audit.rs`（2026-09-19 D9-8 新增） |

## 领域概念（改代码前必读）

- **根目录** = `laew` 二进制所在目录（`current_exe()` 父目录）。数据库 `LsmAgentEmergentWork.db`、编译产物 `./laew` 都在这里。
- **工作目录** = 启动命令时所在目录。Bash/Read/Write 工具的相对路径基准。两者可能不同，勿混淆。
- **当前项目说明文件** = 以**工作目录**为基准按五级链发现：非空 `CLAUDE.md` → 非空 `AGENTS.md` → 非空 `README.md` →（都没有但根目录层有其它 `*.md` 时，程序化分析后**自动生成 `README.md`** 落盘使用）→ 空（不注入）。Yolo 在每个 Session **首次处理**时，把「工作目录路径 + 说明文件内容」包装成带 `<<<LAEW:PROJECT_CONTEXT>>>` 标记的独立 user 消息插入上下文 index 0（标记探测幂等、与用户提示词严格隔离），设计见 `docs/Yolo项目上下文注入/`。TUI 横幅的「项目说明:」行为纯探测展示。
- **接入记录（完整的大模型接入记录）** = `protocol(anthropic|openai) + provider_name + model_name + end_point + api_key` 五元组 + `context_max_size`(上下文最大 Token 数,默认 800K,`0` = 不限制/关闭自动压缩；支持 `800000`/`800K`/`1M` 写法)，存 SQLite `providers` 表，可多条，`is_active` 唯一。存量库打开时自动迁移补列回填默认值。
- **接入点补全**：Anthropic → `{end_point}/v1/messages`；OpenAI → `{end_point}/chat/completions`；尾部 `/` 自动裁剪。
- **运行日志文件（输出 log 文件）**：`--debug` / `--info`（含 `-debug` / `-info` 单横线与 `--DEBUG` 等大小写变体）在工作目录生成 `llaew_YYYYMMDD_HHMMSS.log`（时间戳 = laew 启动时刻，精确到秒，本地时区；已 gitignore）。`--debug` → DEBUG 级（含每轮 LLM 请求元信息/响应思考文本与工具意图全文），`--info` → INFO 级主干事件；实现复用 tracing 双层订阅器（原控制台层行为不变 + 文件层 `src/logging.rs`），埋点覆盖全部 8 角色的感知（任务输入/Agent 会话）/ 决策（Yolo 分类/Plan/Main-Work 拆解/QC 报告）/ 执行（WorkFlow 单元/Context 压缩/SessionContext 摘要/任务收口）/ 思考（LLM 响应文本与 tool_calls）与全部工具调用（名称/参数/结果/耗时，`agent_loop.rs` 中央埋点）。TUI 横幅自第 72 轮起追加「启动时间」行（常显）与「日志文件」行（仅 `--debug`/`--info` 时显示相对化路径 + 级别，`/clear` `/new` 重印横幅仍可见；启动时刻由 main 单点捕获，横幅显示与日志文件名时间戳严格同刻，`TuiLaunch` 传递）。设计见 `tmpPlan/2026-09-17_02-输出log文件功能与全链路日志埋点方案.md`（2026-09-17 第 69 轮）与 `tmpPlan/2026-09-17_04-TUI横幅启动时间与日志文件路径显示方案.md`（第 72 轮）。
- **工具定义协议差异**：Anthropic 用 `tools[].{name,description,input_schema}`；OpenAI 用 `tools[].{type:"function",function:{name,description,parameters}}`（function 风格）。
- **多 Agent 架构(6 角色)**：
  - **Yolo Agent**（`LsmAgentEmergentWork-Yolo`）：入口层，负责目标识别 / 意图识别（每条输入先做 目的→目标→意图 三步分析）/ 任务**三档分类**(simple/medium/hard)/ 失败回流与用户建议；仅持 Read 工具。
  - **Plan Agent**（`LsmAgentEmergentWork-Plan`）：规划层，仅在 hard 任务时启用；持 Read/Write 工具，输出 Markdown 方案到 `plans/{session_id}-{seq}.md`。
  - **Main-Work Agent**（`LsmAgentEmergentWork-Main-Work`）：流程层，接收 medium/hard 任务，拆 WorkFlow 列表；持 Bash/Read 工具。
  - **SubAgent-Work Agent**（`LsmAgentEmergentWork-SubAgent-Work`）：执行层最小单元，每个流程处理单元委派一个 SubAgent；持 Bash/Read/Write 全套工具。
  - **Quality-Check Agent**（`LsmAgentEmergentWork-Quality-Check`）：质检层，每个执行单元完成后必经 QC；可选 Read 工具辅助。
  - **SessionContext Agent**（`LsmAgentEmergentWork-SessionContext`）：会话层，每次任务完成后汇总并写入 `session_memory` 表；无工具。
  - **Debug Agent**（`LsmAgentEmergentWork-Debug`）：调试层，仅在 `-debug` 调试模式下启用；任务结束后对采集的 trace（各 Agent LLM 调用输入输出 / Yolo 分类 / QC 结论 / 耗时与 token / 错误）做评估，产出「任务评估 / 质量报告 / 问题报告(P0-P2) / 优化建议」四章节；无工具。报告写入**根目录** `DebugReport/debug_report_{YYYYMMDD}_{HHMMSS}_{随机6位}.md`（已 gitignore，不入库）。设计见 `docs/Debug模式与DebugAgent设计/01-设计与解决方案.md`。
  - **Compact Agent**（`LsmAgentEmergentWork-Compact`）：压缩层（第 8 角色）；Session 主上下文估算 token（字符/4 +10%）达到当前 Provider `context_max_size` 的 80% 时由 Orchestrator 自动触发，按超出幅度自动选三档压缩率（Light ≤80% / Medium ≈50% / Aggressive ≤20%），LLM 摘要失败降级本地硬截断；保护带项目上下文/历史摘要/已压缩标记的消息与最近 4 条消息；无工具。**溢出兜底（reactive）**：估算可能低估（CJK 2-3 倍），真实溢出（Provider 返回 `prompt is too long` / `context_length_exceeded` 类 400）时由 Agent 循环自动三级恢复——排水（截短超长 tool_result）→ 折叠（历史合并为压缩摘要）→ 暴露（原错误上抛），见 `agent/overflow.rs`（一处包裹、8 角色全生效，全会话恢复预算 4 次）。设计见 `docs/Context设置与自动压缩设计/01-设计与解决方案.md`。
  - **MCP_Window_Use 工具**（2026-09-18 第 84 轮，替代已删除的 WindowUse Agent）：桌面窗口操控能力**降级为 Agent Tools 中的单一 MCP 风格工具**，不再设独立 Agent 角色。单工具 + `action` 枚举分发（`open`/`list`/`find`/`inspect`/`control`/`ocr`/`screenshot`，对应原 WindowOpen/WindowList/WindowFind/WindowInspect/WindowAction/WindowOCR/WindowScreenshot 七工具），结构化 JSON 入参/出参（含 `next_action` 引导）；**双路线**——控件树路线（Windows UIA + Invoke/Value Pattern；macOS AX 需「辅助功能」权限）+ 视觉路线（自绘 UI 控件树为空 → `action=ocr` 词级文本坐标 + `control_action=click_point/type_text_submit` 物理输入坐标动作）。**平台门控**：仅 macOS / Windows 运行时注册进 `builtin_registry()`（SubAgent-Work 全套工具）并同步注入系统提示词使用说明，其余平台不定义。平台差异封闭在 `agent/window/` 驱动层（服务实现原样保留）。任务链路：Yolo 把窗口操控类任务最低判 medium → Main-Work 拆 WorkFlow（桌面 GUI 强信号 → `delegate_to="subagent"`）→ SubAgent-Work 多轮调用 MCP_Window_Use → QC/SessionContext 收口。设计见 `docs/MCP_Window_Use/01-设计与解决方案.md`（平台技术参考 `docs/MCP_Window_Use/MCP_Window_Use_MacOS_技术文档.md` / `MCP_Window_Use_Window_技术文档.md`）。**第 87 轮（2026-09-18）**：屏录探测主判定改 TCC 官方 `CGPreflightScreenCaptureAccess`（根治全屏 CGWindow 假阳性）；OCR/截图失败归因修正 + 能力缓存自愈（`augment_visual_failure`）；`send_keys` 支持 `cmd/ctrl/alt/shift+键` 组合与 a-z/0-9 全量键码（`CGEventSetFlags`，微信 `cmd+f` 搜索联系人）；macOS 无坐标时 `chat_send` 优先 osascript_fallback；Main-Work 提示词注入 11 个合法 action 白名单 + 反伪造验收；SubAgent 反伪造红线 + 长会话必须 `chat_loop`；QC 畸形 JSON 降级链补挂文本关键词；`stable_json_string` 修正为合法 JSON（根治 [tool] 行 `action=?` 与 stage 参数摘要丢失）。**第 88 轮（2026-09-18）**：`osascript_run`/`osascript_fallback` 改 `osascript` argv 直传（根治 shell 单引号包裹致 -2741 语法错误三连败），真实 exit_code + stderr 上抛（根治 BashTool 非零退出静默 ok:true）；chat_send 全路线**前台焦点守卫**（ensure_frontmost 轮询确认，拿不到前台不盲打）+ fallback 路线按窗口 bounds 比例估算点击右下输入框聚焦 + Enter 前二次校验，chat_loop 连续 3 轮焦点失败止损（focus_aborted），根治「用户切窗后消息打进别的软件」；capability_probe 输出一致化（osascript_fallback_available 语义修正、删除 CGWindow 免屏录错误 hint）；TUI `[laew]` 即时冲刷分支推进 current_stage（根治 550s 等待动画滞留旧阶段名）；`[tool]` 行新增 MCP_Window_Use 关键字段摘要（route/verified/rounds/exit_code，`tui/format_brief.rs` 新子模块）。方案见 `tmpPlan/2026-09-18_05-MCP_Window_Use第88轮焦点守卫与osascript根治方案.md`。
  - **MCP_Web_Use 工具**(2026-09-18 第 89 轮,替代已删除的 Chromium-WebUse Agent):浏览器操控能力**降级为 Agent Tools 中的单一 MCP 风格工具**,不再设独立 Agent 角色。单工具 + `action` 枚举分发(`open`/`list`/`close`/`control`/`inspect`,对应原 BrowserNew/BrowserList/BrowserClose/BrowserControl/BrowserInspect 五件套),结构化 JSON 入参/出参;`control` 内层 `control_action` 枚举 34 个写操作(click/right_click/scroll/key_press/input_text/upload_file/new_tab/navigate/wait/eval_js/set_cookie/set_storage/screenshot/drag/focus/dispatch_event 等),`inspect` 的 `info` 枚举 14 个观察维度(console/network/elements/dom/localstorage/sessionstorage/cookies/page_meta/image_urls 等);**无平台门控**——CDP 三平台(Windows/macOS/Linux)行为一致,全平台注册进 `builtin_registry()`(SubAgent-Work 全套工具)并同步注入系统提示词使用说明。CDP 协议驱动(chromiumoxide,Chrome 自动降级 Edge/Chromium/Brave),默认 `--headless=new` 内存无头浏览器 + 一次性 user-data-dir,也可 `connect_url` 接管带 `--remote-debugging-port` 的已开浏览器;未安装浏览器时返回结构化错误码 3001 + 安装引导(不崩溃)。page_id 不透明字符串注册表(`p_`+8 位 hex),点击链接/新开标签页派生页经 `spawned_page_id` 回传;**多轮页面复用**:BrowserManager 页面注册表跨任务存活,SubAgentRunner 入口 `list_pages()` 探测,有存活页面时注入「已打开的浏览器页面」提示块;**出口兜底**:`extract_page_reply_from_session` 从 sub_session 反查 MCP_Web_Use 抓取的真实页面文本(≥200 字符 + UI 占位过滤)追加到终答;`agent_loop` 在 `action=open` 成功后注入 next_steps 四步引导,同 selector 连败 ≥2 次自动追加换姿势提示。统一 JSON 信封错误码(0/1001/2000/2001/2002/3001)。任务链路:Yolo 把网页操控类任务最低判 medium → Main-Work 拆 WorkFlow(统一 `delegate_to="subagent"`)→ SubAgent-Work 多轮调用 MCP_Web_Use → QC/SessionContext/Debug/取消/并行全链路复用。设计见 `docs/MCP_Web_Use/01-设计与解决方案.md`;CDP 驱动层 `src/agent/browser.rs`(平台技术参考 `docs/浏览器CDP工具/Rust操作Chrome浏览器CDP完整技术方案.md`)。
  - **第 90 轮(2026-09-19)**:MCP_Window_Use 新增「鼠标键盘原子能力 + 操作优先级链」。**(a) 优先级链**(Windows 驱动 `windows.rs` act() 重构):控件操作一律 **T1 无障碍 UIA Pattern(Invoke/Toggle/ExpandCollapse/SelectionItem/Value/Scroll)→ T2 Win32 消息(BM_CLICK/WM_SETTEXT/WM_GETTEXT/WM_MOUSEWHEEL/PostMessage 按键,仅原生 HWND)→ T3 物理鼠标键盘(SendInput/CGEvent/xdotool)兜底**,返回文案带 `route=uia/win32_msg/physical` 标注(QC/Debug 可对账);自绘 UI 无 Pattern 无 HWND 自然落穿物理层,零回归。**(b) 鼠标键盘原子能力**:`control_action` 新增 `move_point`(悬停)/`middle_click_point`(中键)/`drag_point`(拖拽 x,y→x2,y2,10 步插值);点击系动作新增 `modifiers` 参数("ctrl"/"ctrl+shift")实现修饰键+鼠标同时操作(ctrl+点击多选/ctrl+拖拽复制)。**(c) 复合 action `input_batch`**:一次调用编排鼠标+键盘+控件树任意顺序步骤(10 种 op,一次前台守卫,`get_text` 读取值回填 `results`,steps≤40/整批≤60s/默认 fail-fast)。**(d) 修复存量 Windows 编译损坏**:第 88 轮 `osascript_exec` cfg 门控漏调用点 + `main.rs` 无条件 `libc::atexit`(libc 仅 unix)+ 第 86 轮 capability 测试按 macOS 写死断言。**(e) 模块拆分**:`ControlAction` 自 `window/mod.rs` 拆出 `window/control_action.rs`;`macos_legacy.rs` 现 2030 行超线,下轮拆分。新增 `input_batch.rs`;系统提示词第 15~17 条;Main-Work 白名单 12 个 action;TUI 摘要 `steps=N/M`。设计见 `docs/MCP_Window_Use/02-鼠标键盘操控与优先级链方案.md`。
  - **第 96 轮(2026-09-20)**:MCP_Window_Use「鼠标键盘同时操作」最后一块拼图——中键修饰键补齐 + macos_legacy.rs 超线拆分。**(a) 中键修饰键补齐**:第 90 轮 `MiddleClickPoint` 三平台均硬编码「无修饰键」,与其他点击变体不一致;本轮为 `MiddleClickPoint` 增加 `modifiers: Option<String>` 字段,三平台(`windows.rs` SendInput / `macos_legacy.rs` CGEvent flags / `fallback.rs` xdotool keydown+mouseup)同步实现修饰键+中键同时操作(ctrl+中键=浏览器新标签打开 / shift+中键=CAD 平移视图)。`ControlAction` 新增 `supports_modifiers()` 辅助。**(b) input_batch 中键步骤透传 modifiers**(`mouse_click button=middle modifiers=?` 不再丢弃)。**(c) 新增 `docs/MCP_Window_Use/04-鼠标键盘同时操作完善方案.md`** 记录设计与验证。
  - **第 93 轮(2026-09-19)**:微信聊天任务实测失败(只激活窗口零输入)全链路根治,方案 `tmpPlan/2026-09-19_06-第93轮微信聊天任务全链路根治方案.md`。**(a) 输入层主根因修复**:无 bracketed paste 的 Windows 终端多行提示词被逐行拆成多次提交(8 条需求只剩标题行进任务)——TUI Enter 提交前 15ms 粘贴突发探测(`tui/input.rs` `drain_paste_burst`/`burst_events_to_text`),队列有后续事件则 Enter 按换行进 D6 粘贴管线不提交,最后一次 Enter 才提交完整提示词,人类单击零误伤。**(b) 断言 OCR 兜底**:run_sequence `assert_text`/`wait_for_text` 在控件树为空(自绘 UI)时自动整窗 OCR 兜底(detail 带 `route=uia/ocr`),`path="/"` 即断言整窗可见文本——根治「断言窗口标题必败 + optional 蒙混」。**(c) 纯 wait 自检**:run_sequence 返回体新增 `wait_only`(全批 wait/断言零 UI 动作)+ next_action 警示「纯等待不构成完成证据」,TUI 摘要 `⚠wait_only`。**(d) QC 反降级**:QC prompt 头部插入「第一性事实 · 用户原始输入」+ 反降级规则(单元职责漏核心动词如「主动聊天」降级为「保活等待」→ Fail+retryable),simple/medium/hard 三链路全透传(`quality.rs::build_unit_qc_prompt`)。**(e) Main-Work 提示词四约束**:长等待走 run_sequence(≤30s)/chat_loop(input_batch wait ≤5s 仅限节奏)、首响应直接输出 JSON 禁止先探索、acceptance 锚定 UI 动作产物不得锚定「进程存活/时间差」。**(f) 第 91/92 轮 GBK 乱码注释 20+ 处清零**。设计文档 `docs/MCP_Window_Use/01-设计与解决方案.md` §12。
  - 由 `MultiAgentOrchestrator` 总编排:用户输入 → 项目上下文注入 → Yolo 分类 → 简单档(SubAgent) / 中档(Main→SubAgent) / 高档(Plan→Main→SubAgent) → Quality-Check → SessionContext 收口。WorkFlow 执行时按 `depends_on` 自动 Kahn 分层(`main_work::topo_layers`),**同层无依赖的 SubAgent 自动并行**(tokio::spawn + Semaphore 上限 3,`OrchestratorConfig::max_parallel_workflows`),跨层严格串行、上游产物按层注入,失败语义与串行一致(fail-fast 回流 Yolo)。
- **执行-验证-修订闭环(第 95 轮)**:WorkFlow 单元 QC 判 `retryable=true` 时先在**单元级局部重试**(仅该单元,注入本单元 QC 结论,不连坐同层姊妹单元),`OrchestratorConfig::unit_retry_budget` 默认 2(单单元最多 3 次尝试,0 = 关闭旧行为);预算耗尽才升级到档位级重试(`max_retry_per_level=3`,retray_hint 回灌 Main-Work)→ Yolo 回流([PREVIOUS_FAILURE] + failure_signals)→ Failed outcome。retry_hint 分层:attempt=0 用档位级 hint / attempt≥1 用本单元 QC issues+suggestion 覆盖 description hint 段(`apply_retry_hint_overlay`,基础任务描述保留不叠加)。简单档与中/高档同等生效(`run_simple` 复用 `run_wf_unit` 消除内联重复)。使用历次累计(成功随 WfUnitOk / 失败随 QualityFailure);QC LLM 错误与用户取消不消耗预算(对齐 H9)。方案 `tmpPlan/2026-09-19_06-单元级局部重试闭环方案.md`。
- **Agent-Context / Agent-Memory**：
  - **Agent-Context**：每个 Agent 独立的实时上下文(消息流 + 状态)，内存态，生命周期 = 当前单元。
  - **Agent-Memory**：每个 Agent 独立的记忆层(输入/输出/错误/产物摘要)，持久化到 SQLite `agent_memory` 表，跨单元/跨 Session 复用。
  - 与 Session 主上下文(用户对话历史)严格隔离。
- **SessionContext 摘要**：每个用户任务完成后 SessionContext 生成 Markdown 摘要写入 `session_memory` 表；Yolo 下次处理时自动注入最近 N 条历史摘要(默认 3),用 `<<<LAEW:SESSION_HISTORY>>>` 标记隔离。
- **AgentProfile**：Agent 身份档案（名称 / 系统提示词 / 工具集），`work_profile()` / `yolo_profile()` 两个工厂函数。
- **Session**：进程内会话，拥有独立 Session ID 与对话上下文（context）；TUI 启动或 `/new` `/clear` 时生成新 Session。**会话持久化（第 95 轮，2026-09-19）**：TUI 每轮任务收口把 transcript **整快照重写**（单事务 DELETE+INSERT，rewind/fork/switch/resume 全 mutation 路径自动一致）到根目录 SQLite `chat_sessions`（索引行：title/turn_count/model_name/updated_at）+ `chat_turns`（轮次：`response` 人类版与 `context_response` 上下文回填版分列）；失败仅告警不打断对话；自动保留最近 50 个会话（新会话落盘同事务淘汰超额）。恢复走 `/sessions` `/resume` / `--resume`（`-c`），重建后保持原 Session ID（session_memory 摘要链连续）、PROJECT_CONTEXT 幂等重注入。设计 `tmpPlan/2026-09-19_02-会话持久化与跨进程恢复方案.md`。
- **请求头**：两协议统一携带 `User-Agent: {AgentName}/{版本} {编译时间}`、`Authorization: Bearer {api_key}`、`X-Session-Id`；Anthropic 请求体 additionally 携带 `metadata.user_id`（含 `device_id/account_uuid/session_id/agent`）。**User-Agent 按"发起请求的 Agent 角色"逐请求注入**（`Agent::run_session_inner` 从 profile 写入 `RequestMeta.user_agent`，8 角色各自携带自身名称，抓包层面可辨识；空值回退客户端构造期默认 UA）——见 `tmpPlan/2026-09-09_08` 方案。

## 架构（src/）

```
main.rs        clap CLI:默认进 tui; -p 单轮; -f 文件提示词; provider 子命令
tui/
  mod.rs       会话外壳:TuiSession 结构 + 生命周期(bootstrap/banner/provider 增删查/分支快照)+ orchestrator 装配 + run()/run_with_debug() 入口(按 ≤1800 行规范自单文件拆分为 dispatch/slash/format 等职责子模块)
  dispatch.rs  输入分发与任务输出:handle_user_input / dispatch_prompt(@提及展开 + 阶段进度协程 + SIGINT 取消)/ emit_debug_report / print_* 家族
  slash.rs     斜杠命令路由:handle_slash + run_theme/run_rewind/run_undo/run_fork/run_branches/run_switch/run_export + print_custom_commands
  provider_screen.rs /provider 子屏桥接:list/add/del 三屏接入 + run_screen_loop(通用 Screen 栈循环,非 TTY 回退 print)
  format.rs    纯函数格式化:任务结果双版本(人类版 format_task_result / LLM 回填版 for_context)/ merge_usage / waiting_line_text / CJK 截断系 / print_help / print_record
  engine.rs    CLI 渲染引擎 —— Screen trait + Frame + 全量重绘 present
  form.rs      通用 Tab 表单状态机(被 ProviderForm 屏复用)
  input.rs     单行输入(主屏用):含行内提示 + 补全(crossterm 原始模式)
  completion.rs 斜杠命令补全引擎(内置 + 自定义命令动态注册)
  commands.rs   自定义斜杠命令(D2):两级目录发现/frontmatter/占位符渲染
  export.rs     会话导出(D8):transcript 记录 + Markdown/JSON 落盘
  branches.rs   对话分支存储(D3):rewind/fork/switch/clear 前自动快照(内存态上限 10)
  theme.rs     ANSI 颜色 / mask_key 脱敏 / attrs·bg·color→ANSI 转换 集中管理
  screen/
    provider_list.rs   /provider list —— Tab 化展示 + 操作按钮
    provider_form.rs   /provider add  —— 5+1 Tab 表单
    provider_del.rs    /provider del  —— Picker + 二次确认
agent/
  mod.rs       agent 域模块注册表 + Agent 结构体/构造器/访问器(协议无关循环的总装配)
  agent_loop.rs Agent 核心循环实现:run_session(Session) → complete → tool_calls → 执行 → tool_result 回填 + 截断续接/溢出恢复
  runtime_hints.rs Agent 循环运行时辅助:runtime hint 拼装/截断判定/首迭代强制工具开关/稳定 JSON 序列化
  orchestrator/ MultiAgentOrchestrator 总编排器目录:mod.rs(结构体+入口+进度通道) / types.rs(共享类型) / pipeline.rs(handle_inner+三档链路) / workflows.rs(分层并行+run_wf_unit) / yolo_reflow.rs(Yolo 分类封装+失败回流) / usage.rs(用量累加) / tests.rs
  main_work/   Main-Work 流程层目录:mod.rs(MainWorkRunner) / spec.rs(WorkFlow 规格模型+宽松反序列化) / delegate.rs(委派推断 GUI 优先) / topo.rs(Kahn 分层+依赖治理) / parse.rs(JSON/Markdown 双通道解析) / tests.rs
  profile.rs   AgentProfile(名称 / 系统提示词 / 工具集) + work_profile()/yolo_profile() + User-Agent
  system_prompt/mod.rs  SystemPrompt 组合与渲染(基础 + 工具说明 + 协议尾缀)
  tools/
    mod.rs     Tool trait + ToolRegistry(有序) + builtin_registry()/yolo_registry()
    bash.rs    BashTool
    read.rs    ReadTool
    write.rs   WriteTool
    mcp_window_use/ MCP_Window_Use 工具目录(仅 macOS/Windows 注册):mod.rs(门面+共享辅助:树预算剪枝/查询扩展/别名匹配/preflight/AX 权限) / query.rs(action=list/find+模糊打分) / open.rs(action=open+应用启动) / inspect.rs(action=inspect/control) / vision.rs(action=ocr/screenshot) / tests.rs
  yolo.rs      YoloRunner 双 Agent 编排器 + TaskLevel + TaskClassification + JSON 解析
  compact.rs   CompactRunner:token 估算 / 三档选档 / 自动压缩触发 / 硬截断降级 / 保护段识别
  window/      窗口操控平台驱动层(MCP_Window_Use 服务实现):mod.rs(模型+WindowDriver trait+工厂) / windows.rs(UIA+Win32) / macos_legacy.rs+macos_axui.rs(AX) / macos_vision_ocr.rs(Vision OCR) / fallback.rs(wmctrl/xdotool)
  browser.rs   浏览器 CDP 驱动层(MCP_Web_Use 的"MCP 服务"实现):BrowserManager 单例(page_id 注册表 + 跨平台浏览器检测 + 引用计数关闭)
  tools/mcp_web_use/ MCP_Web_Use 工具目录:mod.rs(门面+action 分发+open/list/close+page_id 提取) / control.rs(action=control 写操作) / inspect.rs(action=inspect 只读观察) / tests.rs
  overflow.rs  上下文溢出检测(15+ provider 正则)+ 三级恢复(排水/折叠/暴露)
  project_context.rs 项目说明文件五级链发现 + README 自动生成 + 每会话首次注入(幂等标记)
  session_fork.rs 对话 Rewind 轮次扫描(D3):合成消息识别 + 截断边界(供 /rewind /undo /fork /switch)
  workspace.rs 工作区感知(D4):懒刷新快照(git 分支/变更计数/工程类型与工具链建议/顶层结构/6h 最近改动)+ TTL 缓存 + 8 角色 system brief `<<<LAEW:WORKSPACE>>>` + 会话级「工作区快照」段 + TUI 变更对比
session.rs       Session:本机指纹 device_id + Session ID 生成 + 独立对话上下文 context
llm/mod.rs       统一消息模型 + LlmClient trait + RequestMeta + build_common_headers + build_http_client(TLS 三级策略:IP 主机自动放宽自签名证书 / LAEW_TLS_INSECURE 全局开关)
llm/anthropic.rs  Anthropic wire 转换(x-api-key + anthropic-version + metadata.user_id)
llm/openai.rs     OpenAI wire 转换(Bearer)
config/mod.rs    Paths::detect()(根/工作目录) + Db(SQLite CRUD)
error.rs         AgentError(含 YoloParse)
build.rs         注入 LAEW_BUILD_TIME / LAEW_GIT_HASH(供 --version)
```

统一消息模型是关键设计：Agent 循环与工具层永远不接触协议细节，协议差异封闭在 `llm/*` 两个客户端内部。
双 Agent 架构：Yolo 入口层(任务分类/拆解) + Work 执行层(工具调用)，由 `YoloRunner` 编排。

## TUI 界面（独立 CLI 渲染引擎）

### 屏幕拓扑

- **REPL 主屏**：保留 0.1.2 的 `InputHandler` 单行输入 + 斜杠命令补全 + 多轮对话。**D6 大粘贴防护**（2026-09-10）：bracketed paste 整体接收 + 大粘贴（>10 行或 >1000 字符）转 `[粘贴 #N]` marker、提交时展开还原（单份 >10000 字符截断为首尾各 500 + 省略标注）+ 快速输入批量合并（IME/旧终端粘贴逐字重绘优化）。
- **子屏（Modal）**：`engine.rs` 的 Screen 栈接管 `/provider *` 系列，进入 alternate screen + 原始模式，Esc 退回主屏。
- **非 TTY 回退**：stdin 不是终端时（管道 / e2e），子屏与主屏都回退到 print 输出，保证 `run_e2e.sh` 兼容。

### 斜杠命令

| 命令              | 行为                             |
| ----------------- | -------------------------------- |
| `/help` (h, ?)    | 显示帮助                         |
| `/exit` (quit, q) | 退出 TUI                         |
| `/clear` (c)      | 清空对话历史，开启新 Session     |
| `/new` (n)        | 同 `/clear`（开启新 Session）    |
| `/model`          | 显示当前模型                     |
| `/rewind [N]`     | 对话回退（D3）：无参列出全部真实轮次（#编号+时间+预览）；`/rewind N` 回退到第 N 轮之前（context/transcript/累计用量三处一致截断，回退前自动快照存分支）；合成消息（`<<<LAEW:>>>` 标记 / `[PREVIOUS_FAILURE]`）不计轮次，实现 `agent/session_fork.rs` |
| `/undo`           | 撤销最后一轮对话（等价 `/rewind 末轮`） |
| `/fork`           | 从当前对话分叉出新 Session（上下文完整拷贝 + 新 ID，原对话自动存分支） |
| `/branches`       | 列出已存分支（`/rewind` `/fork` `/switch` `/clear` 改动前自动快照；内存态上限 10 个，退出 TUI 失效），实现 `tui/branches.rs` |
| `/switch <name>`  | 切换到指定分支（切换前当前对话自动快照，零丢失） |
| `/sessions` (`hist`) | 列出**跨进程**可恢复的历史会话（第 95 轮：每轮任务收口自动整快照落盘根目录 SQLite `chat_sessions`/`chat_turns`，自动保留最近 50 个）；列表含 #序号/更新时间/轮数/模型/首条预览/session-id |
| `/resume [N\|id前缀]` | 恢复历史会话：重建 context（prompt 展开版 + assistant 上下文回填版）/transcript/累计用量三处一致，保持原 Session ID（`session_memory` 摘要链连续）；恢复前当前对话自动快照存分支；`latest` = 最近一次。CLI 侧 `laew --resume [N\|id]`（短参 `-c`，无参=最近一次，首条输入前生效）与 `laew --sessions`（列出后退出）。实现 `database/chat_store.rs` + `tui/slash.rs::rebuild_from_turns` |
| `/offline` (`status`)| 查看连接状态(Online/Degraded/Offline 三态)与离线队列深度;D13 离线模式 |
| `/cost` (`usage`)    | 查看会话用量与成本估算(D8):累计四类 token / 缓存命中率 / 按内置参考价(2026-09)的成本分解与实记累计;模型无内置价时仅统计 token |
| `/workspace` (`ws`) | 查看工作区快照(D4):git 分支/未提交变更/工程类型与工具链建议/顶层结构/6h 内最近改动;`/workspace refresh` 强制失效 TTL 缓存重采集 |
| `/export [path]`  | 导出当前会话为 Markdown（`.json` 后缀导出 JSON）；默认落工作目录 `laew-export-{时间戳}.md`，同名冲突自动 `-1` 后缀，显式路径已存在拒绝覆盖 |
| `/commands`       | 列出已加载的自定义斜杠命令与来源 |
| `/provider`       | 管理接入记录（默认进入 list 屏） |

### 自定义斜杠命令（D2，2026-09-10）

Markdown Prompt 模板，两级发现：**项目级** `{工作目录}/.laew/commands/*.md` + **用户级** `~/.laew/commands/*.md`（同名用户级优先；内置命令不可遮蔽）。frontmatter 支持 `description` / `argument-hint`（缺失时描述取正文首行截 60 字符）；模板占位符 `$ARGUMENTS`（全量参数）与 `$1`-`$9`（位置参数，`$10` 原样保留）；无占位符但带参调用时末尾追加 `ARGUMENTS:` 块。补全列表内置在前、自定义在后，每行输入前自动重扫（命令文件增删即时生效）。实现见 `src/tui/commands.rs`，方案 `tmpPlan/2026-09-10_01-自定义斜杠命令与会话导出方案.md`。会话导出（D8）transcript 记录在 TUI 层（不动主 Session 上下文），实现见 `src/tui/export.rs`。

### `/provider` 系列交互

| 输入                  | 行为                                                                                                |
| --------------------- | --------------------------------------------------------------------------------------------------- |
| `/provider`           | **默认等价 `/provider list`**，进入 ProviderList 屏                                                 |
| `/provider list` (ls) | ProviderList 屏：5 只读字段 Tab + 操作按钮（设为当前 / 删除 / 返回）                                |
| `/provider add`       | ProviderForm 屏：5+1 Tab 表单（protocol / provider_name / model_name / end_point / api_key / 确认） |
| `/provider use <id>`  | 后台 set_active + 重建 Agent，回到主屏打印 `✓ 已切换`                                               |
| `/provider del`       | ProviderDelPicker 屏 → ProviderDelConfirm 屏（二次确认）                                            |

### Tab 表单交互（ProviderForm）

- 5 个数据 Tab + 1 个确认 Tab；`←` / `→` 环回切换；`Enter` 进入编辑态。
- protocol Tab：Enter 切换 `anthropic ⇄ openai`。
- 文本 Tab：进入行输入，Enter / Esc 退出编辑态（保留修改）。
- 确认 Tab：`[ 确认 ]` / `[ 取消 ]`，左右切换，Enter 触发。
- **API Key 全程脱敏**：浏览态显示 `****<末4位>`；仅进入 Tab 5 编辑态时显示明文。

### 选中效果视觉规范（2026-09-09 起统一遵循）

**核心原则**：所有可聚焦控件（按钮 / 列表行 / Tab 标签 / Choice 选项 / Text 编辑态 / 补全菜单）
的"选中态"与"普通态"必须**一眼可辨**。规范组合：**形状 + 边框 + 颜色 + 字重 + 反白**，按控件类型选用子集：

| 控件类型                             | 形状      | 边框           | 颜色                       | 字重 | 反白    |
| ------------------------------------ | --------- | -------------- | -------------------------- | ---- | ------- |
| **操作按钮** (确认/取消/删除/返回)   | —         | `▶ [ X ] ◀`    | `theme::SELECTED_FG`(Cyan) | Bold | Reverse |
| **列表行** (Picker 记录项)           | `► ` 前缀 | —              | `SELECTED_FG`              | Bold | Reverse |
| **Tab 标签** (表单 label)            | `▸ ` 前缀 | —              | `TAB_FOCUSED_FG`(Cyan)     | Bold | —       |
| **Choice 选中项** (anthropic/openai) | —         | `[ x ]`        | ACCENT                     | Bold | —       |
| **Text 编辑态**                      | —         | `▶ ... ◀` 包裹 | `SELECTED_FG`              | Bold | Reverse |
| **补全菜单选中项**                   | `► ` 前缀 | —              | `HIGHLIGHT_FG`(White)      | Bold | Reverse |

**落地约束**：
- 选中态常量集中在 `src/tui/theme.rs`（`SELECTED_FG` / `SELECTED_ATTRS` / `TAB_FOCUSED_*` /
  `SELECTED_PREFIX` / `TAB_FOCUSED_PREFIX` / `SELECTED_BUTTON_L` / `SELECTED_BUTTON_R`），**禁止屏幕内硬编码**。
- `Cell.attrs: u8` 位掩码（`attr::BOLD | attr::REVERSE | ...`），支持多属性叠加；
  `engine::present()` 逐 cell 检测样式变化并输出 ANSI 序列。
- 不使用 `Blink`（兼容性差且干扰阅读）。
- 新增子屏 / 新控件类型时，必须沿用上述规范；新控件类型先在 `theme.rs` 注册常量再使用。

### 关键约定

- 新增子屏：实现 `engine::Screen` trait，在 `mod.rs::handle_slash` 中路由。
- 不引入新 crate（crossterm 已满足）。
- 子屏不直接写 stdout；通过 `Frame` → `engine::present` 统一绘制。
- 文档：`docs/TUI界面与CLI渲染引擎/`（01-产品设计 / 02-技术设计 / 03-Tab表单与Provider操作设计）。

## 文档地图（docs/）

- `docs/工程初始化方案/` — 从 0 到 1 的分阶段解决方案（架构 / 任务分解 / 技术设计）
- `docs/TUI交互优化与-f命令设计.md` — TUI 自动补全与 `-f` 文件参数的产品和技术设计
- `docs/TUI界面与CLI渲染引擎/` — 独立 CLI 渲染引擎 + Tab 表单 + `/provider` 系列交互（01-产品设计 / 02-技术设计 / 03-Tab表单与Provider操作设计）
- `docs/Agent身份与Session管理/` — AgentProfile / Session / 请求头 User-Agent·Authorization·X-Session-Id / Anthropic metadata.user_id 设计（01-设计与解决方案）
- `docs/Agent系统提示词与工具架构重构/` — 系统提示词独立模块 + 工具迁移到 agent 域 设计文档
- `docs/YoloAgent设计/` — 双 Agent 架构 / Yolo 入口层 / 任务四级分类 / 任务拆解 设计（01-设计与解决方案 / 02-系统提示词设计）
- `docs/Yolo项目上下文注入/` — 项目说明文件五级链发现（CLAUDE.md→AGENTS.md→README.md→自动生成→空）+ 每会话首次注入 + 三步意图识别优化（01-设计与解决方案 / 02-技术实现文档）
- `docs/TUI自动化测试/` — TUI 子屏自动化测试方案:**tmux control-mode** 真 PTY 渲染,命令速查、run_e2e.sh 封装、用例矩阵、断言策略
- `docs/自动化测试-提示词文件列表/` — 10 维度 × 100 组多轮对话测试脚本(知识问答/编码/代码理解/调试/文件处理/电脑使用/软件使用/界面设计/文档规划/laew 元任务),每条 3~5 轮追问,标注预期档位(simple/medium/hard),用于人工/自动化回归与 Yolo 分类验证
- `docs/Debug模式与DebugAgent设计/` — `-debug` 调试模式与 Debug Agent(第 7 角色):trace 采集 / LLM 装饰器 / DebugReport 报告生成 设计与解决方案
- `docs/MCP_Window_Use/` — MCP_Window_Use 桌面窗口操控工具(2026-09-18 第 84 轮,替代已删除的 WindowUse Agent):单工具 action 分发 + 平台门控注册 + 委派链路收缩(01-设计与解决方案,含 84~90 轮增量章节;02-鼠标键盘操控与优先级链方案,第 90 轮;03-连续工作模式设计与解决方案,第 91 轮;04-鼠标键盘同时操作完善方案,第 96 轮——中键修饰键补齐;MacOS/Window 两份平台技术文档)
- `docs/MCP_Web_Use/` — MCP_Web_Use 浏览器操控工具(2026-09-18 第 89 轮,替代已删除的 Chromium-WebUse Agent):单工具 action 分发 + page_id 生命周期 + 能力平移 + 存量数据兼容(01-设计与解决方案)
- `docs/浏览器CDP工具/` — CDP 技术参考:Rust 操作 Chrome 浏览器 CDP 完整技术方案(chromiumoxide 选型 / launch vs connect / BrowserManager 单例 / spawned page / Console·Network 采集)
- `docs/Context设置与自动压缩设计/` — ContextMaxSize 上下文上限(默认 800K,DB 迁移自动补全)+ Compact Agent(第 8 角色)三档自动压缩 设计与解决方案
- `docs/工作区感知与运行时环境注入/` — D4 工作区感知:懒刷新快照(git 分支/变更计数/工程类型与工具链建议/顶层结构/最近改动)+ 8 角色 system brief + PROJECT_CONTEXT 工作区段 + TUI 横幅·`/workspace`·任务后变更对比
- `docs/自签名证书TLS适配/` — IP + 自签名证书 HTTPS 网关适配:TLS 三级校验策略(IP 自动放宽 / LAEW_TLS_INSECURE 全局开关)、跨平台一致性(rustls)、真实端点集成验证(tests/tls_self_signed.rs)
- `docs/协议抓包/` — 各 Agent 真实 HTTP 抓包（RequestBody/ResponseBody）。**codex 走 responses 接口仅参考请求**，其余为主要参考
- `docs/其他Agent工具定义/` — claude-code / codex / hermes / openclaw / open-code / pi / WorkBuddy 等的工具定义，新增工具时先读这里
- `docs/Agent源码调研/` — **15 个外部项目源码**的系统调研与深度分析，**共 80+ 份文档/约 168k 行**（15 份综合文档 + 55 份横向专题；按轮次组织，主文档每轮追加新章节，专题目录按主题持续扩容）。每轮合集见 `专题/专题-第N轮深挖合集.md`。
  - **2026-09-19 第二十轮（当前最新，进行中）**：8 全新维度（**L1931-L2410 / 480+ gap**）——深挖「**执行环境纵深 + 外部集成 + 运行时防护 + 自动化质量**」4 大方向：D15 定时调度与后台自治（claudecode `cronScheduler` 565 行 + deepseek `schedule/` + openclaw `cron-store`）/ D16 Webhook 与事件触发（deepseek `webhook/`+`webhook-github/` 2096 行）/ D17 大对象溢出与产物存储（deepseek `spill/` 2529 行 + claudecode BashTool spill）/ D18 代码审查自动化（atomcode `atomcode-review/` 11 文件：confine/fanout/impact_plan/round_budget/persona/rules/diff）/ D19 TODO 任务清单（deepseek `tool-todo/` projection/invariant + opencode 6 文件）/ D20 Codemode 代码即动作（opencode `codemode/` **6878 行**：interpreter runtime 3465 + tool-runtime 806 + Effect Schema + Sandbox* 值类型 + 三维限制 + OpenAPI 工具自动生成）/ D21 工具调用修复与流式防护（openclaw `tool-call-repair/` **5810 行**：stream-normalizer 1961 + protection-fast-path + ProtectedRange + Harmony/END_TOOL/XML-ish 伪标记）⭐ **最直接对应 laew `json_repair.rs`/`partial_json.rs`**/ D22 云沙箱与远程执行（deepseek `e2b/` 6833 行 + `code-runtime-worker-thread/`）。**已完成**：D20（~530 行 / 60 gap ✅）+ D21（~530 行 / 60 gap ✅）+ 合集；D15-D19/D22 待后续补写。⚠️ **重要修正**：旧「第二十轮候选」（i18n / Web UI / OAuth / Release / DevContainer / CRDT）实为**第九~十二轮已覆盖维度**，已从候选移除。合集见 `专题/专题-第二十轮深挖合集.md`。累计 108+ 维度（90+ 基础设施与协议层 + 16+ 用户交互体验层 + 2 执行环境纵深已完成）。
  - **2026-09-09 第十九轮（已收口）**：6 维度聚焦「用户交互体验层续 + 安全纵深 + 多模态 + A2A + a11y + 离线 + 同步」。⚠️ **补写完成**：D10 多模态输出（688 行 / L1641-L1700，2026-09-19 补写）+ D13 离线模式（621 行 / L1821-L1880，2026-09-19 补写）——此两份原定 2026-09-09 产出但从未落盘，现已补齐。D9/D11/D12/D14 主文档章节各工程已到位。新增 laew gap: L1591-L1930（340+ 个）。合集见 `专题/专题-第十九轮深挖合集.md`。
  - **2026-09-09 第十八轮**：8 份全新专题（**~8,657 行 / ~350 KB**）—— 7 工程主文档各追加一章 + 7 份专题文档 + 1 份跨项目缺口分析。聚焦「**用户交互体验层**」8 维度：D1 @提及系统 / D2 自定义斜杠命令 / D3 对话 Rewind/分支 / D4 文件监视与工作区感知 / D5 工具输出富文本内容渲染 / D6 输入体验工程 / D7 Onboarding/目录信任/主题 / D8 会话导出/Statusline/实时成本 + undici N1-N5 Agent 富内容获取底座。新增 laew gap: L1396-L1590（184 个，**含编号冲突修正**：pi D8 12 gap 从 L1576-L1587 修正为 L1564-L1575，atomcode 预留 L1415-L1425 11 空位）。合集见 `专题/专题-第十八轮深挖合集.md`。
  - **2026-09-06 第五轮**：7 个主文档追加「第五轮深挖补充」章节（atomcode 19 章 / claudecode 17 章 / deepseek-harness 14 章 / openclaw 16 章 / opencode 18 章 / pi 12 章 / undici 专题），新增 2 份横向专题（中断取消与后台任务 / 工具结果回填与消息组装）。
  - **2026-09-06 第六轮**：6 个主文档追加「第六轮深挖」章节（atomcode 第 20 章协议 wire/流式/错误重试 +722 行 / claudecode 第 19 章 40+ Tool 系统统一抽象/权限拦截 +508 行 / deepseek-harness 第 17 章 Goal 域模型/Workflow ralph/SubAgent 11 包 +374 行 / openclaw 第 17 章 Gateway/Harness/Adapter 三层契约/162 Extensions/Lane 调度器/Workshop 自演化 +1370 行 / opencode 第六轮 Effect/Schema DI/LayerNode/Durable Object +1198 行 / pi 第 13 章 Lane 三态/CBOR 帧协议/WriterLease/14 种损坏检测 +1386 行），新增 6 份横向专题（协议调用真实实现增量深挖 13 维度 / SubAgent 调度与并发模型 / Goal 状态机与任务生命周期 / TUI 与终端渲染管线 / Hook 系统与拦截器 / Skill 系统深度对比）。
  - **第七轮（历史归档）**：7 主文档 + 8 横向专题（~10k 行）：文件编辑/代码检索/Git/Bash/多模态/PromptCaching/Schema/WebFetch。合集见 `专题/专题-第七轮深挖合集.md`。
  - **第七轮关键发现（laew gap L16-L25）**：无 Schema 校验、无 token 计数、无 cache_control 断点、无 Grep/Glob 工具、Bash 无超时/进程组/截断/后台、无 Edit 唯一性、无 checkpoint/undo、无多模态、无 WebFetch、无 JSON 修复链 —— 是 laew 从 PoC 升级到生产级 Agent CLI 的核心改造清单，每条都附 Rust crate 建议（`schemars`/`tiktoken-rs`/`grep-searcher`/`similar`/`gix`/`infer`+`image`/`reqwest`+`dns`）。
  - **第八轮（历史归档）**：7 主文档 + 8 横向专题（~13.5k 行 / 640 KB）：Telemetry/Session/Tool 权限/LSP/Hook/Skill/多租户/TUI。合集见 `专题/专题-第八轮深挖合集.md`。
  - **第八轮关键发现（laew gap L26-L37）**：无 LSP/CodeIntel、**无 Session WAL/fsync 紧急 P0**、无 Tool Permission/沙箱、无 OTel/决策审计、无 Skill 系统、无 Plugin Extension API、无 Hook 拦截器、无 cell-based TUI、无 Kitty CSI-u、无 DEC 2026 同步输出、无多租户隔离 —— 是 laew 从「能跑」升级到「生产级 Agent CLI」的核心改造清单，每条都附 Rust crate 建议（`lsp-types`+`tokio::io::duplex`/`rusqlite`+WAL+6 PRAGMA/`landlock`+`seccompiler`/`tracing-opentelemetry`/`tree-sitter`+ 17 字段 frontmatter/`extism`/`ratatui`+`unicode-width`）。
  - **2026-09-07 第九轮**：5 个工程主文档补「第八轮深挖」章节（cc-switch +142 / agent-core +135 / agent-studio +122 / semantica +161 / Switchyard +224 行），共 8 篇全新横向专题（**~84,200 字 / 9,380 行 / ~410 KB**：CrashDump 与错误恢复 1,250 行 / WebUI 与 DesktopApp 1,350 行 / OAuth 认证与多账号 1,280 行 / i18n 国际化 1,150 行 / Release 工程化与 AutoUpdate 1,000 行 / WebSocket 与 SSE 1,100 行 / DevContainer 与容器化 1,100 行 / CRDT 与多端冲突 1,150 行）。合集见 `专题/专题-第九轮深挖合集.md`。
  - **第九轮关键发现（laew gap L38-L78）**：**41 个新 gap**。
  - **第十轮（历史归档）**：15 主文档 + 8 横向专题（~27k 行 / 1 MB），覆盖 CrashDump/WebUI/OAuth/i18n/Release/WS/容器/CRDT。合集见 `专题/专题-第十轮深挖合集.md`。
  - **第十轮关键发现（laew gap L79-L142）**：**64+ 个新 gap**（累计 L1-L142 共 142 个 gap）——
    - **P0 紧急**：无 panic hook（atomcode/claudecode/opencode 4 层防御）/ 无指数退避 / 无熔断器 / 无 OAuth（atomcode Authorization Code + claudecode PKCE + openclaw Device Flow）/ 无 i18n（atomcode Msg enum 225 variant + openclaw 34 locale）/ 无 TUI 中文化 / 无 WebSocket（pi 4-byte length prefix + opencode SSE 5 机制 + openclaw Pre-Auth 预算）/ 无 Dockerfile / 无 AutoUpdate（opencode 8 态 Updater + openclaw appcast.xml Sparkle + claudecode 8 种包管理器）。
    - **P1 重要**：无 Sentry 集成 / 无 Web UI（cc-switch Tauri 2 + claudecode Ink Fork + opencode 5 包 UI 拓扑）/ 无桌面壳 / 无 DevContainer / 无 OAuth 多账号 / 无 i18n 翻译 pipeline / 无 RTL（opencode 5 locale + openclaw 34 locale）/ 无 docker-compose / 无 OAuth refresh 锁（atomcode 跨进程 fcntl 锁）/ 无签名验证（cc-switch 3 种 minisign + openclaw SHA256 锁镜像）。
    - **P2 进阶**：无 CRDT（openclaw Boards 类 CRDT + opencode S3/R2 双后端）/ 无协同编辑（yrs Yjs Rust port）/ 无 Event Sourcing / 无 Session 共享 / 无冲突合并 / 无多租户隔离。
    - **关键工程化细节**：
      - **CrashDump**：atomcode 四层防御（panic hook + signal_restore + circuit-breaker）/ claudecode SentryErrorBoundary + heap dump 1.5GB 自动触发 / opencode Crashpad + 7 天日志自动清理
      - **WebSocket**：pi CBOR strict subset（depth/length/cycle 限制）/ undici 5 状态解析机 + permessage-deflate / opencode OpenAI Responses WebSocket（`responses_websockets=2026-02-06`）
      - **i18n**：atomcode 4 级 locale 发现链 / opencode 3 层 i18n（Astro 20 + App UI 60+ + Desktop 60+）
      - **AutoUpdate**：opencode `allowDowngrade=true` 救命设计 / claudecode PID/mtime 双锁 / atomcode deferred upgrade + circuit-breaker（MAX_APPLY_ATTEMPTS=3）防 boot-loop
    - **推荐 Rust crate**：`human-panic`+`backoff`+`failsafe`+`thiserror`+`keyring`+`oauth2`+`rust-i18n`+`fluent`+`eventsource-client`+`tokio-tungstenite`+`self_update`+`cargo-dist`+`minisign`+`tauri`+`yrs`+`landlock`+`seccompiler`+`ciborium`+`zstd-rs`+`fs2`+`quinn`+`reqwest`+`oauth4webapi`。
  - **2026-09-08 第十三轮（当前最新）**：8 篇全新横向专题（**~14,886 行 / ~605 KB**）：
    1. 本地推理引擎与 GGUF 格式（2,363 行 / 108 KB）—— GGUF v3 完整 13 metadata kv + 36 量化方案 + Ollama 4 大创新 + Rust 绑定四件套（ollama-rs / llama-cpp-2 / candle / mistral.rs）+ Switchyard tier 路由
    2. KV cache 与推理引擎优化（1,881 行 / 88 KB）—— KV cache 内存精算 + PagedAttention + Continuous Batching + Speculative Decoding + RadixAttention + Flash Attention + 推理引擎对比矩阵
    3. GUI 自动化与浏览器控制（1,730 行 / 80 KB）—— openclaw 自研 CDP/Playwright 双栈 6300+ 行 + 6 工程 × 11 维度横向对比 + MCP 桥决策
    4. 操作系统深度交互与内核能力（1,960 行 / 82 KB）—— Landlock ABI v1-v4 + Seccomp BPF 字节码 + eBPF CO-RE + io_uring + OCI Runtime Spec + jiuwenswarm 4182 行生产代码
    5. Agent 评测基准与 Leaderboard（1,742 行 / 79 KB）—— SWE-bench Verified + TerminalBench + WebArena + GAIA + 5 类污染防护 + 公开模型分数对照表
    6. TS/Python/Rust 范式深度对比（1,146 行 / 34 KB）—— 异步运行时 / 错误处理 / 类型系统 / 序列化 / 内存管理 / FFI / 模块系统 / 取消模式 / 流式数据 / 依赖管理 10 维度
    7. Agent DSL 与声明式编程（2,702 行 / 85 KB）—— HCL/KDL/Nickel 5 种 DSL 对照 + LangGraph StateGraph + BAML partial streaming + MiniJinja+Tera + pest+tower-lsp
    8. WebAssembly 沙箱与 WASI（1,362 行 / 46 KB）—— wasmtime Cranelift + Fuel/Epoch 双计量 + WASI Preview 2 完整能力 + WIT + Component Model + 5 大 Agent 用例
    覆盖第十二轮未深入的 8 大新维度。合集见 `专题/专题-第十三轮深挖合集.md`。

  - **2026-09-09 第十四轮**：8 篇全新横向专题（**~9,380 行 / ~385 KB**）：
    1. 数据库优化与存储引擎（1,119 行 / ~45 KB）—— SQLite WAL 配置总表、openclaw 完整 WAL 生命周期、deepseek zstd 压缩+打包编码、opencode Effect DI 迁移
    2. 多级缓存架构与性能优化（1,091 行 / ~42 KB）—— claudecode 7 种应用缓存+cache break 检测、undici RFC 9111 HTTP 缓存、deepseek Write-Behind 一致性、atomcode prefix-stability
    3. 分布式部署与服务发现（1,346 行 / ~55 KB）—— Switchyard Axum+优雅关闭+TLS、OpenClaw Gateway 4 档绑定+Tailscale、ClaudeCode WS 重连策略、AtomCode 信号安全终端恢复
    4. 安全加固与威胁模型（762 行 / ~30 KB）—— STRIDE 总表、claudecode 22 种 bash 检查+tree-sitter、pi 0o600 文件锁、openclaw 14 种脱敏正则、laew 明文 SQLite
    5. 插件热加载与动态扩展（1,478 行 / ~62 KB）—— atomcode Marketplace git 驱动、deepseek Cordis 热替换+node:vm 沙箱、opencode Glob+semver、claudecode 43 工具动态加载、pi jiti 虚拟模块
    6. 错误恢复与容错设计模式（1,149 行 / ~47 KB）—— atomcode 429 所有权分离、claudecode 多策略+持久化重试、undici Symbol 错误码+RetryController 背压、Switchyard Escalation 熔断
    7. 运行时代码分析与自我优化（1,111 行 / ~44 KB）—— claudecode tree-sitter FAIL-CLOSED、opencode Effect Schema 全栈 DI、atomcode tool_args_repair JSON 修复、Switchyard ClassifierContract
    8. 跨项目综合模式与架构演进（1,324 行 / ~60 KB）—— 6 大模式×8 项目对照、统一消息模型/工具注册分离/异步优先共性、中间件链+生命周期钩子+状态机+权限策略 = 最具性价比 4 项升级
    合集见 `专题/专题-第十四轮深挖合集.md`。新增 laew gap: L636-L835（200 个）。

  - **2026-09-09 第十七轮（当前最新）**：7 个工程独立 SubAgent 深度分析 + 跨项目缺口分析（**~7,000+ 行 / ~300 KB**）：
    1. 崩溃恢复与取证（claudecode 五层防御/openclaw 5层纵深/deepseek 三层语义检查点/atomcode 看门狗 ~3,000 行）
    2. 多租户隔离（claudecode 四层隔离/openclaw session-lifecycle/deepseek Cordis Isolate/atomcode Project Bucket ~2,000 行）
    3. RRF 检索（deepseek FTS5+Cursor/openclaw web-search/pi fuzzyMatch/atomcode RecallTool ~1,500 行）
    4. LLM 网关路由（openclaw Failover 16种冻结原因码/claudecode 四提供商/opencode 16+Provider/pi ProviderComposer ~2,500 行）
    5. Pregel 图执行（deepseek Worker线程协议/openclaw session-state-events/pi Lane并发 ~1,500 行）
    6. Skill 生命周期（openclaw 3000+行全栈/claudecode 六阶段/deepseek 注册中心/opencode 五级发现 ~2,500 行）
    7. Agent 预热池（openclaw 80+文件/deepseek LRU/pi SessionWorkerManager ~1,500 行）
    8. Turn 锁（openclaw Command Queue/deepseek Agent Inbox双队列/pi Lane串行化 ~1,500 行）
    9. HTTP 客户端高级实现（openclaw SSRF+DNS钉扎/claudecode mTLS/deepseek NAT64/atomcode 连接池 ~2,000 行）
    10. 安全加固（openclaw 14种prompt注入检测/claudecode 22层Bash/deepseek 凭证分离/atomcode Approval ~2,000 行）
    合集见 `专题/专题-第十七轮深挖合集.md`。新增 laew gap: L1166-L1395+（230+ 个）。
    - **P0 紧急（20 项）**：无 panic hook / 无 CrashDump / 无 graceful shutdown / 无 RRF 混合检索 / 无向量检索 / 无故障转移 / 无 Pregel 图执行 / 无 Agent 池化 / 无 Lease 机制 / 无 Prompt 注入防护 / 无沙箱隔离 / 无 API Key 加密 / 无 SSRF 防护 / 无连接池 / 无重试策略 / 无 Session 级隔离 / 无 Skill 系统 / 无错误审计 / 无 Watchdog / 无会话恢复
    - **P1 重要（40 项）**：无 Watchdog/Repair/Restart Loop / 无数据隔离/权限隔离 / 无 FTS5/无 Snippet / 无负载均衡/无模型回退 / 无 Worker 线程协议 / 无 Skill 注册中心 / 无 LRU 缓存 / 无 Agent Inbox / 无 mTLS / 无 22 层 Bash 检测 等
    - **P2 进阶（30+ 项）**：无 Heap Dump / 无 AsyncLocalStorage / 无 Cordis Isolate / 无 ProviderComposer / 无 PKCE / 无 Telemetry Scrub 等
    - **推荐 Rust crate**：`human-panic`+`ctrlc` / `async-local-storage` / `rusqlite`+FTS5 / `failsafe` / `timely-dataflow` / `tree-sitter`+frontmatter / `lru` / `tokio::sync::Mutex` / `reqwest`+`hyper` / `landlock`+`seccompiler`

  - **2026-09-09 第十六轮**：8 个工程独立 SubAgent 深度分析 + 跨项目缺口分析（**~12,000+ 行 / ~450 KB**）：
    1. 多轮对话恢复与压缩管线（claudecode 七阶段管线/三级恢复×2/cached MC/后台记忆提取/流式工具执行器 ~2,500 行）
    2. 内存加密与 SQLite 全栈基础设施（openclaw Secret Sentinel AES-256-GCM/12 模块 SQLite/14699 行 Hook/租约+Worker 心跳 ~3,000 行）
    3. 扩展加载与 AI 适配器（pi cache-stats/自定义 undici/jiti 加载/23 种扩展事件/OAuth 双检锁/溢出检测/重试策略 ~2,000 行）
    4. 反应式 IoC 与 Workflow 引擎（deepseek-harness Cordis Epoch/Goal 严格事件源/Worker 线程协议/Ralph 脚本/SubAgent 11 包 ~2,500 行）
    5. LLM 协议栈完整抽象层（opencode Route 五层/7 协议/Cache Policy/LLMEvent/LLMError/录制回放 ~2,000 行）
    6. Daemon 守护进程基础设施（atomcode ActiveChatRegistry/空闲看门狗/LiveViewHub 实时同步/OAuth 状态机/权限桥接 ~1,500 行）
    7. HTTP 客户端工程细节（undici SQLite 缓存/SSE 状态机/multipart/WebSocket cork/AbortSignal/FixedQueue ~1,000 行）
    8. 跨项目缺口分析（崩溃恢复/多租户/RRF 检索/LLM 网关/Pregel/Skill 生命周期/预热池/Turn 锁 ~500 行）
    合集见 `专题/专题-第十六轮深挖合集.md`。新增 laew gap: L1036-L1165+（130+ 个）。
    - **P0 紧急（12 项）**：无七阶段上下文管线 / 无 max_output_tokens 三级恢复 / 无 prompt-too-long 三级恢复 / 无 cached microcompact / 无 SQLite WAL 配置 / 无完整性检测 / 无跨进程租约 / 无上下文溢出检测 / 无 provider 重试策略 / 无 Route 五层抽象 / 无 Cache Policy 自动注入 / 无反应式 IoC
    - **P1 重要（45 项）**：无 PermissionContext 工厂 / 无流式工具执行器 / 无内存加密 / 无 Hook 系统 / 无租约心跳 / 无扩展事件系统 / 无 OAuth 凭证存储 / 无 LLMEvent 统一模型 / 无录制回放 / 无实时同步 / 无单飞准入 等
    - **P2 进阶（70+ 项）**：无 SQLite 12 模块 / 无 jiti 加载 / 无 WebSocket cork / 无 FixedQueue / 无设备配对 / 无堆内存自适应 等
    - **推荐 Rust crate**：`aes-gcm`+`secrecy`+`zeroize` / `rusqlite`+`PRAGMA wal` / `wasmtime` / `tokio::sync::broadcast` / `insta` / `jieba-rs`+`sqlite-vec` / `petgraph` / `semver`+`sha2`

  - **2026-09-09 第十五轮**：7 份工程深度分析 + 8 篇横向对比专题（**~7,000+ 行 / ~300 KB**）：
    1. 网络协议深度（480 行）—— HTTP/2多路复用(undici独占)、WebSocket帧协议(claude-code状态机/openclaw帧协议/pi CBOR帧)、TLS握手与mTLS(atomcode三层版本策略/claude-code mTLS全链路/openclaw指纹pinning)、连接池管理(atomcode 15s空闲超时/claude-code池毒化/undici 4种池+SWRR)、SSE流式解析、DNS钉扎与SSRF防护(deepseek-harness独占)、指数退避重试
    2. 编译器前端（603 行）—— claude-code 4436行纯TS Bash Lexer(15种Token类型)、deepseek-harness Typert 3142行analyzer+编译器无关TypeGraph模型、opencode LSP JSON-RPC完整客户端(651行)+多Server按需启动、atomcode tree-sitter 17种语言代码图、pi TypeBox Schema验证+CBOR严格子集
    3. 操作系统内核交互（454 行）—— deepseek-harness自研C11 Landlock launcher(296行)+多平台沙箱链(bwrap/landbelt/Windows ACL)、atomcode信号安全恢复(raw sigaction+async-signal-safe)、openclaw进程树优雅终止+PTY终端+Docker/Podman容器、opencode PTY伪终端封装(ConPTY)+ChildProcess detached进程组
    4. 分布式共识（501 行）—— 7工程无一实现经典Raft/Paxos/Gossip、pi Delta CRDT(1268行)+proper-lockfile+JSONL事务日志、openclaw/opencode/pi三工程实现事件溯源(形态各异)、openclaw SubAgent注册表+重启恢复+Swarm FIFO调度
    5. 机器学习推理（489 行）—— 本地推理全零格局(仅atomcode Ollama和openclaw TTS回退)、推理优化三范式(客户端侧精算/Provider侧感知/可用性优化)、pi/opencode 15+ Provider统一Route抽象、pi唯一实现JSON Schema Strict+Grammar约束采样、deepseek-harness KV Cache对齐compaction
    6. 形式化验证（~450 行）—— 7工程全部无TLA+/Coq/Lean/模型检测、atomcode可执行契约符合性框架(conformance/)最接近形式化验证、claudecode fail-closed安全验证最严密、undici fast-check属性测试唯一P1优先级、opencode Schema驱动Secret检测P0紧急
    7. 图数据库与知识图谱（321 行）—— 7工程全部无Neo4j/RDF/SPARQL、atomcode CodeGraph(12节点+5边+双向邻接表)+BFS+最短路径唯一完整实现、opencode LSP Call Hierarchy图遍历、claude-code teamMemorySync领先
    8. 实时流处理（568 行）—— 无工程实现Kafka/Flink集成、opencode流处理最完整(GlobalBus+Event Sourcing+StreamTransport+Tool Call状态机)、atomcode流重建契约独树一帜、undici底层流控最深(HTTP/2流级背压+WebSocket帧状态机+SendQueue双路径)
    合集见 `专题/专题-第十五轮深挖合集.md`。新增 laew gap: L836-L1035（200 个）。
    - **P0 紧急（~30 项）**：无连接池空闲超时 / 无mTLS / 无指数退避重试 / 无Landlock沙箱 / 无Tree-sitter / 无LSP客户端 / 无进程树优雅终止 / 无PTY终端 / 无契约框架 / 无图数据库 / 无知识图谱 / 无向量检索 等
    - **P1 重要（~100 项）**：无TLS指纹Pinning / 无WebSocket帧协议 / 无SSE严格解析 / 无DNS钉扎 / 无fail-closed AST Walker / 无代码图索引 / 无事件溯源 / 无属性测试 等
    - **P2 进阶（~70 项）**：无HTTP/3 QUIC / 无Raft/Paxos / 无ONNX/TensorRT / 无TLA+/Coq / 无Neo4j / 无Kafka/Flink 等
    - **推荐 Rust crate**：`reqwest`+`hyper`+`native-tls` / `tree-sitter`+`tree-sitter-bash`+`lsp-types`+`tower-lsp` / `landlock`+`seccompiler`+`cgroups-rs`+`io-uring` / `openraft`+`serde_cbor` / `ollama-rs`+`llama-cpp-2`+`ort` / `proptest`+`quickcheck` / `neo4rs`+`faiss-rs`+`petgraph` / `tokio-stream`+`rdkafka`

  - **第十一轮（历史归档）**：8 篇全新横向专题（~30k 行）：Agent 协作与多 Agent 通信协议 / 流式输出与上下文窗口管理 / 错误处理与重试退避与熔断器 / 测试体系与 Eval 基建与录制回放 / 配置系统与多环境管理 / 插件生态与扩展分发与 Hook 系统 / 协议流式翻译与决策溯源与可观测性 / 系统提示词工程与模型适配。合集见 `专题/专题-第十一轮深挖合集.md`。新增 laew gap: L143-L280（138 个）。
  - **第十二轮（历史归档）**：8 篇全新横向专题（~17k 行）：HTTP 客户端连接池重试多路复用与代理链 / 安全防御体系与 Prompt 注入防护与密钥管理 / 模型路由与负载均衡与故障转移 / 数据迁移与版本演进与 Schema 兼容性 / 性能优化与多级缓存与内存管理 / 日志采样与聚合与结构化日志管道 / CLI 框架与命令分发与自动补全 / 状态持久化与序列化与快照恢复。合集见 `专题/专题-第十二轮深挖合集.md`。新增 laew gap: L281-L403（123 个）。
    - **P0 紧急**：无 HTTP 超时设置 / 无重试退避 / 无熔断器 / 无 OAuth PKCE / 无 Keychain 凭证管理 / 无 SQLite 迁移系统（无 SCHEMA_VERSION）/ 无结构化日志（仍 println!）/ 无 API Key 日志脱敏 / 无 SSRF 防护 / 无 Shell 自动补全 / 无 Schema 版本管理 / 无 Writer Lease 乐观锁 / 无 8 种路由算法 / 无熔断器三态 / 无应用层 LRU / 无 SQLite WAL 模式。
    - **P1 重要**：无 4 jitter 退避 / 无 DNS pinning / 无 Landlock / 无 oauth refresh 锁 / 无 mimalloc / 无 OTLP / 无 Token 计数 / 无 clap_complete / 无 r2d2 连接池 / 无 Effect Schema / 无 Effect Durable Object / 无双 pass scrub / 无 SHA256 链 / 无 traceparent / 无 apply_migrations() / 无 SAVEPOINT / 无预热 / 无指标 / 无告警。
    - **P2 进阶**：无 Scope-driven eviction / 无 BubbleWrap / 无 CRDT / 无 multi-account UI / 无 cargo audit / 无 SBOM / 无 CBOR 二进制帧 / 无 Seccomp / 无 netns / 无 cgroup 资源限制。
    - **关键工程化细节**：
      - **HTTP 客户端**：atomcode SwappableClient 池毒化 / openclaw DNS pinning + PinnedDispatcherPool LRU / undici RetryHandler 548 行 + Dispatcher compose 8 拦截器 + GOAWAY 重排 / opencode Effect 类型安全 4 层
      - **安全防御**：jiuwenswarm JiuwenBox BubbleWrap + Landlock ABI 1-5 + Seccomp + netns + cgroup 5 层 / claudecode 27 Hook + Permission 5 态 + Keychain/DPAPI / atomcode `<system-reminder>` 边界 + 双 pass scrub（key=value + token 形状）
      - **数据迁移**：hermes-agent 30 级链式迁移 / cc-switch 17 级链式 + SAVEPOINT + pre-migration 备份 / openclaw 50+ state migration + doctor 诊断 / opencode Effect 数组式迁移
      - **性能优化**：TencentDB PipelineWorker 60 协程 + 锁冲突指数退避 + RRF over-retrieve ×3 / opencode Scope-driven eviction / pi Lane 三态 + Writer Lease Fence / atomcode 内容寻址 sha256 修订号
      - **日志体系**：atomcode `tracing` + scrub.rs 双 pass + tracing-bunyan-formatter / opencode Effect + OTLP / openclaw pino 5 档采样 + redact 数组
      - **CLI 框架**：clap derive (atomcode) / Commander + 强类型 (claudecode openclaw) / yargs + Effect (opencode) / 自研 parseArgs (pi) / Tauri IPC (cc-switch)
      - **状态持久化**：opencode effect-drizzle-sqlite + Durable Object + Effect Schema / atomcode daemon 持久化 / pi Session Backends / semantica BiTemporalFact
      - **模型路由**：Switchyard 8 种路由算法 + Prometheus + JSONL 路由日志 / openclaw 16 种 FailoverReason + 时间驱动冷却 / claudecode 429 三层决策 / atomcode swap-aware 热换装
    - **推荐 Rust crate**：`reqwest`+`backoff`+`failsafe`+`keyring`+`secrecy`+`zeroize`+`aes-gcm`+`oauth2`+`ring`+`fs2`+`landlock`+`seccompiler`+`bollard`+`tracing`+`tracing-subscriber`+`tracing-appender`+`tracing-bunyan-formatter`+`tracing-opentelemetry`+`opentelemetry-otlp`+`metrics`+`clap_complete`+`dialoguer`+`indicatif`+`lru`+`moka`+`mimalloc`+`parking_lot`+`dashmap`+`arc-swap`+`refinery`+`serde`+`bincode`+`rmp-serde`+`ciborium`+`r2d2_sqlite`+`governor`+`tiktoken-rs`+`ammonia`+`schemars`。
  - **第十三轮关键发现（laew gap L404-L635）**：**232 个新 gap**（累计 L1-L635 共 635 个 gap）——
    - **P0 紧急（~60 项）**：无 Ollama 集成 / 无 cache_read 命中率展示 / 无 Landlock FFI / 无 Seccomp BPF / 无 cgroup 限制 / 无 wasmtime 沙箱 / 无 Mock LLM / 无 schemars / 无 BAML partial streaming / 无 CancellationToken。
    - **P1 重要（~120 项）**：无 MiniJinja 模板 / 无 figment 配置 / 无 pest parser / 无 tower-lsp / 无 statig 状态机 / 无 LangGraph StateGraph / 无 WIT/Component Model / 无 Fuel/Epoch 计量 / 无 PagedAttention / 无 Continuous Batching / 无 cargo-dist / 无 Cargo Dist。
    - **P2 进阶（~50 项）**：无 candle / 无 eBPF / 无 io_uring / 无 Pkl / 无 OpenTelemetry Collector / 无 Datalog / 无 Rete / 无 BubbleWrap / 无 WebGPU。
    - **关键修正**：atomcode 已实现 Ollama 集成 1170 行（NDJSON decoder + tool_call id 合成）；jiuwenswarm JiuwenBox 5 层沙箱（Landlock FFI 296 + Seccomp BPF 336 + cgroup 528 + bwrap 503 + daemon 2719）；Switchyard PyO3 + pyo3-asyncio 工业级跨语言桥；openclaw 自研 CDP/Playwright 双栈 6300+ 行。
    - **推荐 Rust crate**：`ollama-rs`+`llama-cpp-2`+`candle-core`+`landlock`+`seccompiler`+`cgroups-rs`+`aya`+`wasmtime`+`wasmtime-wasi`+`extism`+`wit-bindgen`+`schemars`+`minijinja`+`pest`+`tower-lsp`+`statig`+`mockito`+`proptest`+`criterion`+`headless_chrome`+`radix_trie`。

  - 覆盖架构/多轮对话/Context/循环架构/工具调用/记忆系统/Workflow/目标意图识别/目标规划/Agent协作调度/Yolo/质检/任务拆解/分类/MCP/SKILL/沙箱设计/权限管控/LLM网关/协议翻译/上下文注入/决策溯源/流式渲染/错误容错/遥测/持久化/测试Eval/成本控制/提示词工程/配置系统/插件生态/HTTP客户端/协议调用实现/Agent间通信协议/中断取消/工具结果回填/协议 wire 真实实现/SubAgent 并发/Goal 状态机/TUI 渲染管线/Hook 拦截器/Skill 一等公民/Effect DI 拓扑/CBOR 二进制帧/Lane 三队列/WriterLease fence/文件编辑补丁/代码检索/Git checkpoint/Bash PTY/多模态/PromptCaching/Schema 校验/Web 检索/Telemetry/Session 持久化/Tool 权限沙箱/LSP/IDE 集成/Skill Workshop/多租户团队记忆/终端控制序列/CrashDump/WebUI/OAuth/i18n/Release/WebSocket/容器化/CRDT/Agent协作/流式输出/错误处理/测试体系/配置系统/插件生态/协议翻译/系统提示词/模型路由/负载均衡/故障转移/熔断器/健康检查/配额限流/多区域部署/路由算法/Provider抽象/连接池/重试退避/HTTP2多路复用/代理链/TLS配置/拦截器/超时取消/连接健康检查/性能优化 等 **90+ 维度**（第15轮新增：网络协议深度/编译器前端/OS内核交互/分布式共识/ML推理/形式化验证/图数据库/实时流处理；第16轮新增：多轮对话恢复/压缩管线/内存加密/SQLite全栈/Hook系统/租约引擎/反应式IoC/LLM协议栈/扩展加载/录制回放/守护进程基础设施）。

  **15 份 Agent 综合文档**（每份合并了源码调研/深度分析/核心机制/第二轮/第三轮/第四轮共 3-8 轮内容，去重压缩 50-90%）：
  | 文件                        | 语言/技术          | 核心亮点                                                                                                                                                                                                                   | 行数       |
  | --------------------------- | ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------- |
  | `atomcode.md`               | Rust               | L0/L1/L2 分层 + cargo feature gating + kernel 内部 trait + MCP 7 子模块 + CodeIntel 七件套 + daemon + tuix TUI + 协议 wire + 流式 + 错误重试 + **第 22 章 LSP/Telemetry/OAuth/Daemon (+1063 行)**                          | ~5,175 行  |
  | `claudecode.md`             | TypeScript/Bun     | 四级压缩管线 + 27 种 Hook + 5 种执行器 + 40+ 工具 + Ink Fork + Bridge 远程控制 + Tool 系统 40+ 工具统一抽象 + 并发执行 + 权限拦截 + **第 21 章 Bridge远程控制/Skill一等公民/i18n/Release工程化 (+2124 行)**                | ~7,755 行  |
  | `deepseek-harness.md`       | TypeScript         | Cordis Everything-is-a-Plugin + Fiber epoch + 30+ 核心模块 + Typert 协议 + ACP/A2A/E2A/A2UI + Goal 域模型 + Workflow ralph + SubAgent 11 包 + **第 19 章 跨语言互操作/Evals/多平台/Native (+642 行)**                      | ~3,577 行  |
  | `openclaw.md`               | TypeScript         | Gateway/Harness/Adapter 三层契约 + 162 extensions + 双向 MCP + Lane 调度器 + Workshop 自演化 + **第 19 章 Custodian Skills/多端部署/Taxonomy/Security (+700 行)**                                                          | ~5,291 行  |
  | `opencode.md`               | TypeScript/Bun     | Effect + Schema 全栈 DI + LayerNode 拓扑 + 34 包 + enterprise Durable Object + R2 + Effect 异步运行时 + LayerNode DI + Durable Object + **第 20 章 Enterprise Durable Object/多端 UI/HTTP Recorder/Slack 集成 (+1102 行)** | ~5,143 行  |
  | `pi.md`                     | TypeScript         | lane 并发 + 一等公民 Skill + 二进制帧协议(CBOR) + WriterLease fence + 14 种损坏检测 + Lane 三态 + 三队列驱动 + per-file 排他锁 + **第 16 章 Coding Agent/AI 路由/Session Backends/OTLP Telemetry (+908 行)**               | ~4,684 行  |
  | `hermes-agent.md`           | Python             | 859 MB + 6 前端共享 AIAgent + 38 provider + CompressionCommitFence + FTS5 + Trigram                                                                                                                                        | ~1,632 行  |
  | `undici.md`                 | JavaScript         | Node.js 官方 HTTP 客户端(非 Agent) + Dispatcher + HTTP/2 + llhttp WASM + 8 拦截器 + Mock 录制回放 + **第 15 章 HTTP/3 QUIC/代理链 SOCKS/TLS 证书/Cookies 重定向 (+853 行)**                                                | ~10,040 行 |
  | `cc-switch.md`              | Tauri 2+Rust+React | 8 款工具适配 + 熔断器三态 + thinking_rectifier + MCP SSOT + 17 schema 迁移 + WebDAV/S3                                                                                                                                     | ~1,293 行  |
  | `agent-core.md`             | Python             | openJiuwen Core SDK + ReAct + ContextEngine + 多类型记忆 + PermissionEngine + Pregel + Rails + OTLP                                                                                                                        | ~1,630 行  |
  | `agent-studio.md`           | Python             | 一站式 Agent 平台 + Pregel cba 消减 + DSL 双向转换 + 5 种 MCP 传输 + BubbleWrap + Seccomp                                                                                                                                  | ~1,005 行  |
  | `jiuwenswarm.md`            | Python             | 多 Agent 协作 + Leader-Teammate + A2A/ACP/E2A/A2UI + SkillDevPipeline 12 阶段 + SwarmFlow DAG + JiuwenBox                                                                                                                  | ~1,395 行  |
  | `semantica.md`              | Python             | 图原生 AI 基础设施 + Context Graph + Rete + Datalog + SPARQL + W3C PROV-O + BiTemporalFact                                                                                                                                 | ~1,716 行  |
  | `Switchyard.md`             | Rust               | NVIDIA LLM 网关 + 协议 IR + ContentBlock::Unknown + TranslationEngine + 7 种路由算法 + PyO3                                                                                                                                | ~1,807 行  |
  | `TencentDB-Agent-Memory.md` | TypeScript+Python  | 团队记忆系统 + L0-L3 管线 + SkillCore 6写4读 + InjectionPipeline 8注入点 + RRF 混合检索                                                                                                                                    | ~1,569 行  |

  **30 份横向专题**（位于 `专题/` 子目录）：
  - `专题-12Agent全面对比深度分析.md` — 13 项目 18 维度横向对比总表 + 4 大新维度专题 + P0-P3 路线图
  - `专题-Context上下文管理深度分析.md` — Context 压缩管线横向对比(4级/3级/2级/可插拔/投影)
  - `专题-MCP架构深度分析.md` — MCP 传输层/工具注册/资源/认证/双向支持横向对比
  - `专题-Skill系统深度分析.md` — Skill 文件格式/注册/加载/触发/内置数量横向对比
  - `专题-SubAgent与多Agent架构深度分析.md` — 多 Agent 编排模式/上下文传递/并发/通信横向对比
  - `专题-任务拆解与分类深度分析.md` — 任务分类/拆解粒度/失败回流横向对比
  - `专题-质检机制深度分析.md` — 质检形式/触发/检查项/自动修复/失败回流横向对比
  - `专题-多轮对话与循环架构深度分析.md` — agentic loop 驱动/终止/续轮/流式耦合横向对比
  - `专题-工具调用深度分析.md` — 工具定义/Schema/消息流/并发/权限拦截横向对比
  - `专题-记忆系统与上下文注入深度分析.md` — 5 仓库深度对比,含 L0-L3 管线 + RRF + 9 注入点 + W3C PROV-O + P0-P5 路线图
  - `专题-Workflow设计深度分析.md` — workflow 定义/编排拓扑/执行引擎横向对比
  - `专题-Yolo目标意图识别与目标规划深度分析.md` — 目标识别/意图/生命周期/规划生成横向对比
  - `专题-多Agent协作与权限管控深度分析.md` — 5 仓库深度对比,含 TeamAgent/Leader-Teammate/BubbleWrap/Seccomp/HITL/Fork 上下文/P0-P2 路线图
  - `专题-沙箱设计深度分析.md` — 进程/文件/网络/能力/资源隔离 + laew 现状(零沙箱) + P0-P2 路线图
  - `专题-权限管控深度分析.md` — 三态策略/Bash 黑名单/路径白名单 + laew 现状(零校验) + P0-P2 路线图
  - `专题-LLM网关与协议翻译深度分析.md` — Switchyard/agent-studio/cc-switch 协议 IR + 翻译 + 路由算法 + 熔断器
  - **`专题-第十二轮-HTTP客户端连接池重试多路复用与代理链深度对比.md`** — 第十二轮 7 工程 × 10 维度 HTTP 客户端深度对比（连接池/重试/HTTP2/代理/TLS/拦截器/流式/超时/健康/性能），含 laew 现状基准线、20 个 HTTP 专项 gap（H1-H20）、P0/P1/P2 改造路线图、可直接使用的 Rust 参考实现
  - `专题-横向对比深度分析合集.md` — 横向专题索引(15 专题)
  - `专题-第二轮深挖合集.md` — 8 份深挖合集索引 + 三大共性模式 + 6 周 P0 路线图
  - `专题-第三轮深挖合集.md` — 15 份深挖合集索引 + 5 大共性模式 + P0-P2 路线图
  - `专题-第三轮-流式输出与终端渲染管线深度分析.md` — SSE chunk/partial JSON/cell-diff/16ms 帧率节流
  - `专题-第三轮-错误处理重试与容错降级深度分析.md` — 10+ 熔断计数器/durable 重试/eligible_routing_fallback
  - `专题-第三轮-可观测性遥测与决策审计深度分析.md` — 5 级 opt-out/GROUPING SETS/隐私脱敏
  - `专题-第三轮-会话持久化与崩溃恢复深度分析.md` — 双后端/WriterLease/parentUuid 树/JSONL 撕裂修复
  - `专题-第三轮-测试体系与Eval基建深度分析.md` — vitest-evals/eval.py/录制回放
  - `专题-第三轮-成本控制与Token统计深度分析.md` — 微美分计价/6 tier 价格表/cache miss 量化
  - `专题-第三轮-系统提示词工程真实对比深度分析.md` — 14 个模型家族变体/9 种失败行为建模
  - `专题-第三轮-配置系统与多环境管理深度分析.md` — 8 层发现链/5 源+安全隔离/5 级 ApplyPolicy
  - `专题-第三轮-插件生态与扩展分发深度分析.md` — Cordis Fiber 六态/fail-closed/ExtensionAPI 30+事件
  - `专题-第四轮-Anthropic与OpenAI协议调用真实实现对比.md` — 7 仓库 × 8 维度协议调用真实实现对比
  - `专题-第四轮-流式协议翻译与Agent通信决策溯源深度分析.md` — SSE↔内部事件 + A2A/ACP/E2A/A2UI + 决策溯源
  - `专题-第五轮-中断取消与后台任务深度分析.md` — 取消原语三家族/双路信号/钩子决策/孤儿修复/后台任务 横向对比
  - `专题-第五轮-工具结果回填与消息组装深度分析.md` — 三档截断/50KB 行业默认/协议中立 ToolResult/cache 6 provider 分发/孤儿修复 5 策略
  - **`专题-第六轮-Anthropic与OpenAI协议调用真实实现深度对比.md`** — 第四轮协议对比的姊妹篇：13 个深度主题（Reasoning 往返 / Cache 断点上限 / Streaming Stop 差异化 / Tool Call id 命名约束 / 多模态 / Structured Output / 计费 / OAuth 双模式 / Compaction Replay / Tool Schema 投影 / SSE 字节优化）+ laew 漏点清单 L1-L15 + 完整度 54/100 评分
  - **`专题-第六轮-SubAgent调度与并发模型深度对比.md`** — 6 项目 SubAgent 抽象 + 父-子通信 + 并发（lane/semaphore/task queue）+ 权限继承降级 + 失败处理 + 持久化 + 嵌套深度 + 资源限制
  - **`专题-第六轮-Goal状态机与任务生命周期深度对比.md`** — 7 项目（atomcode/claudecode/deepseek-harness/openclaw/opencode/pi/jiuwenswarm）Goal/Task/Plan 状态机 + 持久化 + 嵌套子状态 + 并发状态 + 计划审批 + 回退
  - **`专题-第六轮-TUI与终端渲染管线深度对比.md`** — 6 项目 TUI 渲染模型（重绘/retained cell-based/行级 string diff/Web DOM+anser）+ 对象池 + DEC 2026 + CJK 宽度 + worker thread 渲染 + Kitty CSI-u
  - **`专题-第六轮-Hook系统与拦截器深度对比.md`** — 6 项目 Hook 注册 + 触发时机 + 决策能力 + 失败处理 + 沙箱（claudecode 27 种 Hook / atomcode LifecycleHooks / deepseek agent/request-error / openclaw pre/post tool）
  - **`专题-第六轮-Skill系统深度对比.md`** — 6 项目 Skill 文件格式 + 注册 + 加载 + 触发 + 内置数量 + Workshop 自演化（atomcode catalog hook / claudecode 6 源 / openclaw 52 skill + Workshop 闭环 / pi 一等公民 + SDK 替换）
  - **`专题-第七轮-文件编辑与补丁策略深度对比.md`** — Edit 工具 old_string 唯一性 + 4 级匹配 ladder + GBK 编码识别 + 8 段 failure hint + Read dedup + 9 项目逐项剖析
  - **`专题-第七轮-代码检索与索引深度对比.md`** — ripgrep/ignore/globset 内核 + tree-sitter 符号提取 + RRF 混合检索 + max-columns 500 + head_limit 250 + mtime 排序
  - **`专题-第七轮-Git集成与变更回滚checkpoint深度对比.md`** — checkpoint 3 架构对比（claudecode cp/opencode shadow git/atomcode git-dir）+ undo/快照 + gix + shadow repo + SQLite 元数据
  - **`专题-第七轮-Bash命令执行与PTY进程管理深度对比.md`** — persistent shell + 进程组 kill + 30K/150K 截断 + 落盘回灌 + POSIX 信号速查表 + setsid + Job Object
  - **`专题-第七轮-多模态与文件处理深度对比.md`** — 图片全链路 magic byte → resize → base64 → image block + PDF/Notebook + 协议差异代码对比 + sharp 渐进压缩
  - **`专题-第七轮-PromptCaching与Token预算控制深度对比.md`** — cache_control 断点放置 + 4-chars/token 启发式 + 阈值 80%/保留 16% + Shadow-Price 协议 + cache_edits 主动删除
  - **`专题-第七轮-结构化输出与Schema校验深度对比.md`** — OpenAI strict 归一化 + $ref 展平 + 8 段 JSON 修复链 + schemars/jsonschema + atomcode repair.rs
  - **`专题-第七轮-Web检索与网络访问深度对比.md`** — WebFetch 两阶段 + url_safety.py 纵深 SSRF + 私有 IP/CGNAT 阻断 + 两层 DNS 防御
  - **`专题-第七轮深挖合集.md`** — 第七轮 8 维度索引 + L16-L25 gap 清单 + 与前 6 轮关系
  - **`专题-第八轮-Telemetry可观测性与决策审计深度对比.md`** — 6 工程 OTel 三栈 + atomcode 双 pass scrub + 决策审计 3 段式范本 + W3C traceparent + gen_ai.client.token.usage histogram
  - **`专题-第八轮-Session持久化与崩溃恢复深度对比.md`** — fsync 4 严格度分层（temp+fsync+rename+fsync(parent_dir)）+ WriterLease 乐观锁 + JSONL 撕裂修复 + pi 真正双后端架构 + opencode 6 PRAGMA
  - **`专题-第八轮-Tool权限策略引擎与沙箱设计深度对比.md`** — 5 态状态机 + 4 维规则引擎 + macOS SBPL 60+ sysctl + Landlock ABI 1-5 协商 + Windows WRITE_RESTRICTED + per-workspace SHA256 SID
  - **`专题-第八轮-LSP与IDE集成与CodeIntel深度对比.md`** — 自研 JSON-RPC vs vscode-jsonrpc 两派 + atomcode CodeIntel 7 件套 + LRU 跨 turn 诊断去重 + -32801 ContentModified 退避 + IDE 反向集成
  - **`专题-第八轮-Hook拦截器与PluginExtensionAPI深度对比.md`** — Hook ≠ Extension 两层抽象 + claudecode 27 种事件 + openclaw 153 bundled 插件 + atomcode plugin_hook_set_hash 内容寻址 + Cordis 6 态生命周期
  - **`专题-第八轮-Skill一等公民与Workshop自演化深度对比.md`** — jiuwenswarm 12 阶段 SkillDevPipeline + openclaw Workshop 5×4 状态机 + 5 档 proposal 预算 + scanner 5 类规则 + TencentDB SkillCore 6 写 4 读
  - **`专题-第八轮-多租户与团队记忆与组织级共享深度对比.md`** — TencentDB-Agent-Memory L0-L3 管线 + RRF 公式 (k=60) + 9 注入点 + 5 档 AssetVisibility + opencode Actor 四元 + jiuwenswarm Leader-Teammate
  - **`专题-第八轮-TUI渲染管线与终端控制序列深度对比.md`** — 渲染模型 4 档 + cell-based retained + Kitty CSI-u 三档哲学 + DEC 2026 三种语义 + CJK/emoji 宽度算法 + worker thread 渲染
  - **`专题-第八轮深挖合集.md`** — 第八轮 8 维度索引 + L26-L37 gap 清单 + 22 个 laew gap 完整覆盖度 72/100 + 与前 7 轮关系
  - **`专题-第九轮深挖合集.md`** — 第九轮 8 维度横向专题（CrashDump/WebUI/OAuth/i18n/Release/WebSocket/容器化/CRDT）+ L38-L78 gap 清单 + 与前 8 轮关系
  - **`专题-第十三轮深挖合集.md`** — 第十三轮（当前最新）8 大新维度索引 + L404-L635 共 232 个 laew gap 完整覆盖度 + 95+ 维度全景 + 累计 14,886 行 / ~605 KB
  - **`专题-第十三轮-本地推理引擎与GGUF格式深度对比.md`** — GGUF v3 完整剖析 + Rust 绑定四件套 + Ollama 4 大创新 + 量化决策矩阵
  - **`专题-第十三轮-KVcache与推理引擎优化深度对比.md`** — KV cache 数学精算 + PagedAttention + Continuous Batching + Speculative Decoding
  - **`专题-第十三轮-GUI自动化与浏览器控制深度对比.md`** — CDP 50 域 + Playwright 架构 + a11y tree 三段 + 视觉定位 SOM
  - **`专题-第十三轮-操作系统深度交互与内核能力深度对比.md`** — Landlock + Seccomp + cgroup + eBPF + io_uring + 三层沙箱叠加
  - **`专题-第十三轮-Agent评测基准与Leaderboard深度对比.md`** — SWE-bench Verified + TerminalBench + WebArena + 5 类污染防护
  - **`专题-第十三轮-TSPythonRust范式深度对比.md`** — 三语言 10 维度对比（异步/错误/类型/序列化/内存/FFI/模块/取消/流式/依赖）
  - **`专题-第十三轮-AgentDSL与声明式编程深度对比.md`** — HCL/KDL/Nickel 5 种 DSL + LangGraph + BAML + MiniJinja + pest
  - **`专题-第十三轮-WebAssembly沙箱与WASI深度对比.md`** — wasmtime + Fuel/Epoch + WASI Preview 2 + WIT + Component Model
  - **`专题-第十轮深挖合集.md`** — 第十轮 15 主文档全部追加新章节（~26,940 行 / ~1 MB）+ 8 大新维度全景 + L79-L142 gap 清单 + 142 个 laew gap 累计完整覆盖度
  - **`专题-laew实现进度对照表.md`** — ⭐ **实现进度中央账本(改代码前必查)**:知识库 gap(H/L 编号)→ 状态(✅/🟡/⏳/⛔)→ 实现位置 → 完成轮次 的对照索引,防止多轮任务重复实现;已实现的 gap 在对应专题原文条目旁有 ✅ 标记(如第十二轮 H1/H2/H3/H10、第七轮 L17)。实现新功能后必须回填此表 + 就地标记

- `docs/Agent架构对比与参考.md` — 7 个项目(6 外部 + laew)的横向对比报告,含 10 维度对比表、15 个跨项目设计模式、laew 借鉴路线图(P0/P1/P2)、反模式警示

## 自动化测试

测试分三层,放在 `testReport/` 下:

| 层             | 入口                            | 用途                                                                             |
| -------------- | ------------------------------- | -------------------------------------------------------------------------------- |
| 单元测试       | `cargo test`                    | Rust 函数级覆盖(模块、解析、转换、工具)                                          |
| 端到端(CLI)    | `bash testReport/run_e2e.sh`    | mock LLM,跑 `-p` / `provider add                                                 | list | use | delete` / 协议 wire 校验 / **项目上下文注入(说明文件五级链,5b 节)** / TUI 管道冒烟 / **TUI 子屏 tmux 自动化** |
| TUI 子屏自动化 | `testReport/run_e2e.sh` 第 8 节 | tmux control-mode 真 PTY 渲染,验证 alternate screen + raw mode + Screen::title() |

### TUI 自动化:**优先使用 tmux control-mode**

`src/tui/mod.rs::run` 对 `atty()` 做了分流:

- **TTY**(包括 tmux 内) → `InputHandler` 全交互(原始模式 + alternate screen + 子屏栈)。
- **非 TTY**(管道 / 重定向) → 行读取回退,**子屏走 print 输出,不是真实渲染**。

> 因此:`/provider list`、`/provider add`、`/provider del` 等**子屏行为必须用 tmux**。
> 管道冒烟仅适合主屏纯文本命令(`/help` `/model` `/new` `/exit`)。

核心命令速查(完整封装见 `run_e2e.sh` 第 8 节,设计见 `docs/TUI自动化测试/`):

```bash
# 1) 起后台会话并启动 TUI,固定 100x30
tmux new-session -d -s laew_e2e -x 100 -y 30 "$LAEW"
# 2) 发送按键(整串字面量必须 -l)
tmux send-keys -t laew_e2e -l "/provider list"
tmux send-keys -t laew_e2e Enter        # 回车
tmux send-keys -t laew_e2e Escape       # Esc 退子屏
# 3) 抓取面板到 stdout(不带 -e 剥离 ANSI,便于 grep)
SCREEN=$(tmux capture-pane -p -t laew_e2e)
# 4) 断言:echo "$SCREEN" | grep -F -q "/provider list"
# 5) 调试:tmux attach -t laew_e2e  可肉眼回放
# 6) 收尾:tmux kill-session -t laew_e2e
```

扩展指引(新增子屏断言):

1. 在 `src/tui/screen/*` 找到 `fn title() -> &str`,title 字符串本身就是断言锚点。
2. 在 `run_e2e.sh` 第 8 节 `texpect "<title>" "..."` 即可。
3. 若断言失败,报告自动 dump 当前面板(带 `|` 前缀),便于排查。
4. CI 环境需 `apt-get install -y tmux`;缺 tmux 时整节 SKIP,不影响其它 9 节通过。

## 约定

- 注释、CLI 文案、文档一律中文；代码标识符英文。
- **单源码文件 ≤ 1800 行**：超过必须按功能/业务/架构维度拆分模块；临界文件（≥ 1700 行）新增代码优先落到职责子模块，防止越线。**拆分规范**（目录化拆分统一按此执行，最新方案 `tmpPlan/2026-09-17_01-单文件1800行超标拆分重构方案.md`）：
  - 单文件超线 → 转同名目录（`xxx.rs` → `xxx/mod.rs` + 职责子模块）；`mod.rs` 承载结构体定义 + 构造器/入口 + `pub use` 再导出，**外部 `crate::…::xxx::Yyy` 路径零改动**；
  - 原私有项搬入子模块标 `pub(super)`（可见域 = 本目录子树，与拆分前单文件作用域等价），由 `mod.rs` 私有 `use` 重导入供兄弟模块 `use super::*` 取用；
  - 代码**逐行机械搬移零改写**（不重构逻辑/注释），仅新增模块文档头与 `use super::*`；测试搬 `tests.rs`（mod.rs 声明 `#[cfg(test)] mod tests;`），拆分前后测试数必须对账一致；
  - 高速增长文件（近几轮每轮 +50 行以上）在破线前预防性拆分。已按此规范拆分：`src/tui/`（分层）、`src/agent/`（agent_loop + runtime_hints + tests 平铺拆分）、`agent/orchestrator/`、`agent/main_work/`、`agent/tools/window/`。
- 新工具：在 `src/agent/tools/` 建同名模块实现 `Tool` trait，注册进 `builtin_registry()`（Work Agent）或相应 registry，Schema 参考 `docs/其他Agent工具定义/`。
- 新协议：实现 `LlmClient` trait + `client_from_record()` 增加分支，不改动 agent 层。
- 新 Agent 类型：实现 `AgentProfile`（独立名称/系统提示词/工具集），在 `YoloRunner` 或相应编排器中接入。
- 测试报告输出到 `testReport/`（命名 `e2e-<时间戳>.txt` / `验证报告-<日期>.md`）；临时计划放 `tmpPlan/`（已 gitignore）。
- `laew`、`*.db`、`tmpPlan/`、`target/` 均不入库（见 .gitignore）。
