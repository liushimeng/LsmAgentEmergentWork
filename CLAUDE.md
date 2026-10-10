# CLAUDE.md — LsmAgentEmergentWork

供 AI Agent Tools（Claude Code / Codex / Hermes / OpenCode / pi / OpenClaw 等）自动加载的工程入口说明。
**模块化、功能化导向**：本文档只描述"工程当前长什么样"与"开发约定"，不再累加"在第 X 轮新增了 Y"类开发日志。
**配套镜像**：[`AGENTS.md`](./AGENTS.md) 保留为兼容性入口，内容与本文同步维护。

## 工程是什么

由 LLM 驱动的 Rust Agent CLI（二进制名 **`laew`**）。支持 **Anthropic**（anthropic-messages）与
**OpenAI**（openai-completions）双协议，**多 Agent 架构**（8 角色 + 三档难度），
内置 Bash / Read / Write / Edit / Glob / Grep 六个核心工具 + MCP_Web_Use 浏览器操控
+ MCP_Window_Use 桌面窗口操控 + MCP_Use 通用 MCP 服务调用 + SubAgent 自感知动态委派
+ Skill 渐进式披露 + 决策审计 + 离线模式 + 上下文自动压缩 + 工作区感知。
TUI 多轮对话 + `-p` 单轮模式 + `-f` 文件提示词模式 + `--debug`/`--info` 运行日志模式
+ `--resume`/`--sessions` 历史会话持久化 + `--mcp` 通用 MCP server 接入管理。

TUI 支持：
- 斜杠命令自动补全（Tab 补全 + 行内提示）
- @ 文件提及（`@路径` / `@"带空格"` / `@路径#L10-20` 行区间，输入 @ 后 Tab 实时路径补全、目录可钻取；命中文件内容以 `<<<LAEW:ATTACHMENTS>>>` 附件块自动注入上下文，实现见 `src/agent/attachments.rs` + `src/tui/mention.rs`）
- 自定义斜杠命令（`.laew/commands/*.md` + `~/.laew/commands/*.md` 两级发现）
- 自定义子 Agent 类型（`.laew/agents/*.md` + `~/.laew/agents/*.md` 两级发现）

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
./laew --resume [N|id]                    # 恢复历史会话后进入 TUI;短参 -c,无参=最近一次;首条输入前生效
./laew --sessions                         # 列出持久化历史会话后退出(不进 TUI)
./laew provider add|list|use|delete ...
./laew mcp add|list|del|test ...          # 管理 MCP server 接入记录(通用 MCP 服务调用)
```

注意：crates.io 在本机网络较慢，已在 `~/.cargo/config.toml` 配置 rsproxy.cn 镜像。

## 环境变量

| 变量 | 取值 | 行为 |
| ---- | ---- | ---- |
| `LAEW_TLS_INSECURE` | `1`/`true`/`yes`/`on` | TLS 全局宽松：所有 endpoint 跳过证书校验（仅调试） |
| | `0`/`false`/`no`/`off` | TLS 全局严格：所有 endpoint 严格校验（安全基线） |
| | 未设置（默认） | 自动模式：endpoint 主机为 IP（IPv4/IPv6）时自动跳过证书校验，域名主机仍严格校验。适配 IP + 自签名证书的内网/自建 HTTPS 网关；仅跳过校验，TLS 加密不降级；rustls 纯 Rust 实现，Windows/macOS/CentOS/Ubuntu 行为一致。设计见 `docs/自签名证书TLS适配/01-设计与解决方案.md` |
| `LAEW_ALLOW_PRIVATE_ENDPOINT` | `1` | SSRF 防护放行私网/loopback endpoint（本地 Ollama / 局域网 / mock 测试 provider 用；默认拦截，见 `src/agent/safety/url_safety.rs`） |
| `LAEW_BASH_UTF8` | `1`/`true`/`yes`/`on` | Bash 工具为子进程注入 UTF-8 环境（`PYTHONUTF8=1`/`PYTHONIOENCODING=utf-8`/`LC_ALL=C.UTF-8`），消除 Windows 区域设置(GBK)导致的 python/coreutils 输出乱码；默认关闭。行尾(CRLF)不受影响，精确 diff 场景脚本仍需 `reconfigure(newline=...)`，见 `src/agent/tools/bash.rs` |
| `LAEW_AMBIGUOUS_WIDE` | `1`/`true`/`yes`/`on` | 歧义宽度字符（`✓ · … → ║` 等 East Asian Ambiguous）按 **2 列**参与宽度计算（默认关闭=按 1 列）。只改计算不改渲染，供把盒线渲染成双宽的老终端（部分 Windows conhost 中文字体 / xterm `-ctwidth`）校正对齐；见 `src/tui/textfit.rs::ambiguous_wide` |
| `LAEW_LOG_CLIP` | 正整数（默认 `4000`） | `--debug`/`--info` 运行日志文件中单字段（LLM 思考文本/工具参数/结果等）的截断长度（字符数）；`0`/非法值回退默认。见 `src/logging.rs` |
| `LAEW_AUDIT` | `off`/`0`/`false`/`no` | 关闭决策审计写入（默认开启）。开启时 5 个决策点（Yolo 分类 / Plan 规划 / Main-Work 拆解 / QC 判定 / Compact 压缩）各追加一条结构化 JSON 行到根目录 `AuditTrail/audit_{session_id}.jsonl`（已 gitignore），记录「输入上下文→决策结论→决策依据」三段式 + 耗时/token/扩展字段，全字段脱敏截断，fail-open 不影响主流程。见 `src/agent/decision_audit.rs` |
| `LAEW_PARALLEL_TOOLS` | `off`/`0`/`false`/`no` | 关闭「同批只读工具并发执行」，回退到全串行。默认开启：同一 LLM 响应里**连续的** `parallel_safe` 调用（Read/Glob/Grep，段长 ≥2）并发执行，`tool_result` 仍按原序回填。见 `src/agent/tool_exec.rs` |
| `LAEW_MAX_PARALLEL_TOOLS` | 正整数（默认 `4`） | 单批并发上限；`0`/非法值回退默认。对齐 AtomCode `ATOMCODE_MAX_PARALLEL_TOOLS=4`（Claude Code 用 10，laew 单元迭代预算小取保守值）。见 `src/agent/tools/mod.rs::max_parallel_tools_from` |
| `LAEW_REACT_GUARD` | `off`/`0`/`false`/`no` | 关闭 ReAct 循环守卫（无进展软提醒与 doom_loop 止损均不生效；`doom_loop_repeats` 仍统计，可观测性不丢）。默认开启：进展键 =「工具名 + 参数稳定 JSON + 结果摘要」三者全同即无进展，第 2 次软提醒 / 第 3 次宽限轮强提醒 / 第 4 次止损终止。见 `src/agent/loop_guard.rs` |
| `LAEW_MCP_ENABLED` | `off`/`0`/`false`/`no` | 关闭通用 MCP 服务调用（`MCP_Use` 工具注册与提示词同时归零，严格向后兼容）。默认开启：Agent 可经 `MCP_Use` 调用 `laew mcp add` 配置的外部 MCP server 工具/资源。见 `src/agent/tools/mcp_use/mod.rs` |
| `LAEW_SELF_SPAWN` | `off`/`0`/`false`/`no` | 关闭**自感知动态子 Agent**（默认开启）。关闭时不注册 `SubAgent` 工具、不注入自感知提示词段、不建运行时（工具面/提示词/耗时与改造前完全一致）。见 `src/agent/self_awareness.rs` |
| `LAEW_TARGET_ANCHOR` | `off`/`0`/`false`/`no` | 关闭**任务锚点**全部四层（L1 澄清门 / L2 伪澄清单元阻断 / L3 工具门 6001 / L4 目标一致性 QC 门 + target_drift 信号）。严格回退到第 127 轮行为，与 `LAEW_MCP_ENABLED` / `LAEW_SELF_SPAWN` 同构。默认开启：实测 2026-09-24 事故（多行粘贴被截断后 Agent 自行改派到无关站点被判「✅ 成功」）的根治方案；见 `tmpPlan/2026-09-24_03-任务锚点与防目标漂移根治方案.md`、`src/agent/safety/target_anchor.rs` |
| `LAEW_TARGET_ANCHOR_BLOCK` | `off`/`0`/`false`/`no` | 仅关闭 L3 工具级**阻断**（`MCP_Web_Use` 的 6001 不返回，仅记 warn），保留 L1 / L2 / L4 与信号打标（观察模式）。默认开启阻断。适合灰度期先观察不拦截 |
| `LAEW_BROWSER_MODE` | `headed`/`hidden`/`new_headless`（大小写不敏感，别名 `head`/`with_head`/`visible`/`inprocess`/`cdp_only`/`headless`；旧 bool 写法 `false`/`0`/`off` ≡ `headed`，`true`/`1` ≡ `new_headless`） | 浏览器启动模式**显式覆盖**（第 139 轮起真正接线）。不设时按 `has_gui_session()` 判定：有 GUI 会话 → **默认可见 `headed`**；无 GUI 会话（`CI`/`SSH_*`/Linux 无 `DISPLAY`·`WAYLAND_DISPLAY`/macOS `launchctl managername` 非 `Aqua`）→ 自动回退 `hidden`。`LAEW_FORCE_HEADLESS=1` 可无条件强制无头。见 `src/agent/browser_mode.rs` |
| `LAEW_WEB_EVIDENCE` | `off`/`0`/`false`/`no` | 关闭**网页取证纪律**硬闸门（默认开启）。命中「Bash 网络取证命令 + 命令中的 host 落在任务锚点域内 + 工具面含 MCP_Web_Use」时直接拒绝并推回 MCP_Web_Use；`curl localhost:8080` 等非任务目标一律放行。见 `src/agent/safety/web_evidence.rs`、`docs/MCP_Web_Use/04-网页取证纪律.md` |
| `LAEW_WEB_OVERLAY` | `off`/`0`/`false`/`no` | 关闭**可视化模式页面蒙层与人工操作拦截**（第 141 轮 legacy；第 143 轮起为 `LAEW_WEB_GUARD` 三档级联的末端输入，off 系 ≡ 缺省档 open）：headed 浏览器页面覆盖半透明蒙层（`pointer-events:none` 纯视觉）+ CDP `Input.setIgnoreInputEvents` 输入拦截——人工可实时观看页面但不可点击/操作（防交叉操作）；Agent 输入类动作自动「先解后锁」，截图自动避让蒙层，`request_human` 默认 `unlock_page=true` 提问期间解锁页面、收口后复锁。仅 headed 生效；open 单次覆盖 `overlay=false`；运行时开关 `control(set_overlay)`。见 `src/agent/browser_overlay.rs`、`docs/MCP_Web_Use/05-可视化模式蒙层与人工操作拦截.md` |
| `LAEW_WEB_GUARD` | `locked`（缺省）/ `open` | **页面管控三档**（第 143 轮）的缺省档位：headed 可视化浏览器中人工对页面的操作权限——`locked` 屏蔽模式（半透明蒙层 + CDP 输入拦截，人工可看不可点，防交叉操作）/ `open` 非屏蔽模式（人工可直接操作，状态条明示「页面开放」）/ `partial` 部分屏蔽模式（**环境变量不支持**，须在 open 参数 / `set_guard` 传 `allow_selectors` 白名单或 `block_selectors` 黑名单）。级联：open 显式 `guard` > legacy `overlay` 布尔 > 本变量 > `LAEW_WEB_OVERLAY` > locked。运行时切换 `control(set_guard)`；`guard_note` 写页面状态条引导人工（人工登录先行流）。见 `src/agent/browser_overlay.rs`、`docs/MCP_Web_Use/07-页面管控三档模式与人工交互设计.md` |
| `LAEW_WEB_HITL_SCOPE` | `off`/`0`/`false`/`no` | 关闭**HITL 凭证区域部分放行**（第 144 轮）：`request_human(unlock_page=true)` 提问期间跳过凭证区探测，恒整页临时切 open（回退第 143 轮行为）。默认开启：优先 partial + `allow_selectors` 白名单挖洞，只放行账号/密码/验证码等人工必填输入区（显式传参 > 自动探测），探测不到才整页 open；提问期间主 frame 导航自动重放放行态。见 `src/agent/tools/mcp_web_use/unlock_zone.rs`、`docs/MCP_Web_Use/08-浏览器进程保护与凭证区域部分放行.md` |
| `LAEW_SUBAGENT_MAX_DEPTH` | 0..=3 | 动态子 Agent 嵌套层数上限，默认 `1`（子 Agent 为叶子，不能再启动）。`0` = 完全禁止（等价关闭）。 |
| `LAEW_SUBAGENT_MAX_PARALLEL` | 1..=8 | 会话级并发槽位，默认 `3`（对齐 atomcode `Semaphore(3)`）。 |
| `LAEW_SUBAGENT_MAX_TOTAL` | 1..=64 | 会话级累计启动预算，默认 `8`（防 token 失控；耗尽返回信封 `2001`）。 |
| `LAEW_SUBAGENT_MAX_ITERATIONS` | 4..=32 | 单个子 Agent 迭代上限，默认 `12`。 |
| `LAEW_SUBAGENT_TIMEOUT_SECS` | 10..=3600 | 单个子 Agent 墙钟超时，默认 `300`（超时记 `status=timeout`）。 |
| `LAEW_SUBAGENT_PERSIST` | `off`/`0`/`false`/`no` | 关闭动态子 Agent **运行记录持久化**（默认开启）。关闭后不写不读 SQLite `subagent_run` 表，`SubAgent(action="history")` 返回 `code=0 + persist:false + 空列表`（工具面与提示词不变；`LAEW_SELF_SPAWN=off` 时本就零写入）。见 `src/config/subagent_run.rs` |
| `LAEW_SUBAGENT_RUN_KEEP` | 0..=20000 | 启动期保留的最新运行记录条数，默认 `500`（`0` = 不清理）。TUI bootstrap 与 `-p`/`-f` 单轮模式启动时各执行一次 trim（fail-open）。 |
| `LAEW_CHROME_DEBUG_PORT` | 正整数（默认 `9222`） | 复用已登录浏览器（第 142 轮）的 CDP 调试端口探测覆盖：`open(reuse_existing=true)` 依次尝试 `localhost`/`[::1]`/`127.0.0.1` 该端口（Chrome 154+ 的 DevTools HTTP 端点只服务 IPv6 loopback 连接）；非法值回退默认。见 `src/agent/browser_reuse.rs`、`docs/MCP_Web_Use/06-复用已登录浏览器会话.md` |

## 领域概念（改代码前必读）

- **根目录** = `laew` 二进制所在目录（`current_exe()` 父目录）。数据库 `LsmAgentEmergentWork.db`、编译产物 `./laew` 都在这里。
- **工作目录** = 启动命令时所在目录。Bash/Read/Write 工具的相对路径基准。两者可能不同，勿混淆。
- **当前项目说明文件** = 以**工作目录**为基准按五级链发现：非空 `CLAUDE.md` → 非空 `AGENTS.md` → 非空 `README.md` →（都没有但根目录层有其它 `*.md` 时，程序化分析后**自动生成 `README.md`** 落盘使用）→ 空（不注入）。Yolo 在每个 Session **首次处理**时，把「工作目录路径 + 说明文件内容」包装成带 `<<<LAEW:PROJECT_CONTEXT>>>` 标记的独立 user 消息插入上下文 index 0（标记探测幂等、与用户提示词严格隔离），设计见 `docs/Yolo项目上下文注入/`。TUI 横幅的「项目说明:」行为纯探测展示。
- **接入记录（完整的大模型接入记录）** = `protocol(anthropic|openai) + provider_name + model_name + end_point + api_key` 五元组 + `context_max_size`(上下文最大 Token 数,默认 800K,`0` = 不限制/关闭自动压缩；支持 `800000`/`800K`/`1M` 写法)，存 SQLite `providers` 表，可多条，`is_active` 唯一。存量库打开时自动迁移补列回填默认值。
- **接入点补全**：Anthropic → `{end_point}/v1/messages`；OpenAI → `{end_point}/chat/completions`；尾部 `/` 自动裁剪。
- **运行日志文件（输出 log 文件）**：`--debug` / `--info`（含 `-debug` / `-info` 单横线与 `--DEBUG` 等大小写变体）在工作目录生成 `llaew_YYYYMMDD_HHMMSS.log`（时间戳 = laew 启动时刻，精确到秒，本地时区；已 gitignore）。`--debug` → DEBUG 级（含每轮 LLM 请求元信息/响应思考文本与工具意图全文），`--info` → INFO 级主干事件；实现复用 tracing 双层订阅器（原控制台层行为不变 + 文件层 `src/logging.rs`），埋点覆盖全部 8 角色的感知（任务输入/Agent 会话）/ 决策（Yolo 分类/Plan/Main-Work 拆解/QC 报告）/ 执行（WorkFlow 单元/Context 压缩/SessionContext 摘要/任务收口）/ 思考（LLM 响应文本与 tool_calls）与全部工具调用（名称/参数/结果/耗时，`agent_loop.rs` 中央埋点）。TUI 横幅追加「启动时间」行（常显）与「日志文件」行（仅 `--debug`/`--info` 时显示相对化路径 + 级别，`/clear` `/new` 重印横幅仍可见；启动时刻由 main 单点捕获，横幅显示与日志文件名时间戳严格同刻，`TuiLaunch` 传递）。
- **工具定义协议差异**：Anthropic 用 `tools[].{name,description,input_schema}`；OpenAI 用 `tools[].{type:"function",function:{name,description,parameters}}`（function 风格）。
- **网页取证纪律（第 131 轮）**：网页任务的证据只能来自 `MCP_Web_Use`——`MCP_Web_Use(action=open)` 返 `2001`/`3001` 时**如实报告不可达并停止**，禁止改用 `curl/ping/nc` 等 Bash 网络命令探测任务目标站点（实测事故：28 次 Bash 对 8 次 MCP_Web_Use，Bash 抓的内容 QC 无法与浏览器轨迹对账，与伪造同质）。三层设防：(1) 提示词 `src/agent/system_prompt/web_evidence.rs` 第 20 条（仅注入持 MCP_Web_Use 的 SubAgent-Work，并修正了 Main-Work「任务前提验证」原本鼓励 `curl` 验证 URL 的三处表述）；(2) 运行时硬闸门 `src/agent/safety/web_evidence.rs`（五条件全中才拦：开关未关 + 是 Bash + 工具面含 MCP_Web_Use + 命令命中网络取证程序 + 命令里的 host 落在任务锚点域内；host 抽取含**裸 IP 字面量**通道，`nc -z 10.255.159.58 20122` 这类无 scheme 的内网诊断才拦得住）；(3) `LAEW_WEB_EVIDENCE=off` 全关回退。设计见 `docs/MCP_Web_Use/04-网页取证纪律.md`。
- **人工介入可见性（第 131 轮）**：`request_human` 的「该走人工」不再只写在提示词里，OCR 报错/空文本（`control(screenshot,ocr=true)` 与 `inspect(info=ocr)`）与 `inspect(info=blockers)` 命中阻断时，响应附 `next_action="request_human"` + 完整 `human_assist` 载荷（ready/reason/message/options/可复制的 call/反伪造 rule）；`4001 Unavailable` 附 `human_ui_diagnostics{gui_enabled,platform,reason_hint,env_switch}` 供排障「为什么没弹窗」；`-p`/`-f` 单轮模式新增 250ms 轮询协程排空 `AssistEvent` 并打弹窗通知行（此前弹窗照弹、应答照回填，但终端零输出）。见 `docs/MCP_Web_Use/02-人工介入与窗口可视化方案.md` §5.0/§5.1.1/§5.3。
- **人工介入弹窗可用性与取消收口（第 132 轮）**：macOS 弹窗由 NSAlert+runModal **重写为自绘 NSWindow**（`NSApp.run()` 常规事件循环），根治实测四问题——文案可鼠标选中/⌘C 复制（NSTextView）、输入框 first responder + 输入法可正常输入、连点输入框不再误关弹窗、倒计时每秒真实刷新 + 超时自灭；**验证码图片展示**：`request_human` 附图（显式 `params.image_path` 优先，`reason=captcha` 自动 CDP 视口截图，不受宿主录屏权限影响），payload 新增 `image_path`，弹窗内直接渲染、TUI 打印路径，Windows ps1 同步（ReadOnly TextBox + PictureBox）；**Ctrl-C 取消收口**：取消后 Debug 报告跳过 LLM 评估（`ReportMeta.interrupted` → 本地骨架秒级落盘，实测原评估阻塞 4 分钟且零反馈），stage 打印协程与 HITL 行读接入取消 token（TUI 不再卡「正在取消当前任务...」）。见 `docs/MCP_Web_Use/03-人工介入弹窗原生UI方案.md` §11(历史沿革)。
- **人工介入弹窗全量重构为 Rust 原生子进程（第 137 轮）**：实测「⛶ 最大化后全部按钮无响应」再次复现（第 132/133/134 轮已修三类同类问题），判定为 JXA/PowerShell 脚本层的结构性缺陷（布局双账本 + 回读桥接值 + 编译器不可见）。弹窗重构为 **`laew __hitl-dialog <payload.json>` 隐藏子进程**（与 `__browser-watchdog` 同构）：macOS 走 AppKit FFI（`macos_dialog.rs`，手写 objc runtime 绑定 `objc_msgSend` 按签名包装，零新 crate，dlopen 装载 Foundation/AppKit），Windows 走 Win32（`windows_dialog.rs`，**WM_SIZE 全量重排**根治最大化）。布局改为 Rust 纯函数单一真源（`layout_frames`/`content_height` 同源参数化 + 单测锁死预算对账），最大化/还原只用自身设定值重排、永不回读窗口 frame；payload/result JSON 契约与 Hub/TUI/工具信封零改动；**js/ps1 脚本层与动态加载机制（`LAEW_HUMAN_UI_SCRIPT`/`.laew/human_ui/`）全部删除**。真机冒烟验证：answer（输入+提交）/timeout（超时自灭）两路径、倒计时刷新、输入框焦点均正常。见 `docs/MCP_Web_Use/03-人工介入弹窗原生UI方案.md`。
- **人工介入弹窗文本选择与复制（第 139 轮）**：此前 `human_ui/` 全目录**零 `NSMenu`**，输入框的 ⌘C/⌘V 靠 NSTextField 在 responder chain 上隐式接住 `copy:`/`paste:`（无菜单栏、无右键菜单），`make_label` 又从不调 `setSelectable:`（`NSTextField` 默认不可选），标题/时间轴/倒计时/图片提示一个字都复制不出来；实测反馈正是「输入框不好复制粘贴、其它文案不能选中复制」。三处改造：(1) `install_main_menu` 装应用菜单 + **编辑菜单**（剪切/复制/粘贴/全选 ⌘X/⌘C/⌘V/⌘A），用标准 responder action 沿 first responder 派发，输入框与说明区同时生效，**必须在建控件前安装**；(2) `make_label` 统一 `editable=NO + selectable=YES`（只读可选文本），输入框补显式 `setSelectable:YES`，删掉图片提示那句反向的 `setSelectable:false`，NSTextView 补 `setVerticallyResizable:YES` 让长说明真能滚；(3) **「📋 复制」一键复制**——`DialogPayload::copy_text()`（两平台共用的纯函数：kind_label + 说明 + 页面 URL + 页面ID + 提出/截止时刻 + 候选选项）写系统剪贴板（NSPasteboard / Win32 `SetClipboardData`），按钮标题变「✓ 已复制」后由**既有 1s 定时器**回滚（不额外起定时器）。为什么必须有一键复制：倒计时行**每秒重绘**，人工拖选过程中就被冲掉。底部按钮行新增一钮（`ZOOM_W` 110→104，新增 `COPY_W=92`），`layout_frames`/`content_height` 的布局单一真源性质不变，单测锁死底部四按钮同行对齐且互不重叠。Win32 剪贴板路径本机无法交叉编译验证（MSVC 被 `ring` 阻断），改用独立 scratch crate 只链 `windows 0.58` 校验签名——借此发现并修正两处真实错误（`GlobalAlloc` 在 0.58 返回 `Result<HGLOBAL>`；`CF_UNICODETEXT` 在 `Win32_System_Ole` 而非 `DataExchange`）。见 `docs/MCP_Web_Use/03-人工介入弹窗原生UI方案.md` §4.3/§13。
- **TUI 粘贴只显示一遍（第 131 轮）**：多行/超长粘贴在**粘贴瞬间**已回显前 4 行预览，提交时不再全量重复回显原文，改为回显输入行 marker 形态 + 一行「已按原文完整发送 N 行 / M 字」；纯键盘输入路径完全不变。见 `src/tui/paste.rs::plan_submit_echo_with_paste`。
- **终端守卫（第 136 轮）**：`src/tui/term_guard.rs`。macOS 实测事故——终端窗口/标签被关后
  pty master 关闭，slave 的 `read()` 恒返回 EOF，而 crossterm 0.27 的 `event::poll(timeout)`
  与 `event::read()` 在该状态下**都会永久 hang**（`mio.rs:95-120` 内层循环不检查超时、
  `Ok(0)` 不 break），表现为 100% CPU 空转；又因 `shutdown.rs` 为 SIGHUP/SIGTERM 注册的
  handler 只写标志位不触发退出（注册 handler 还会抑制内核默认终止动作），主线程卡在
  `read()` 里回不到 `tui/mod.rs` 的检查点，导致**进程永不退出且 `kill` 无效**（只有 `kill -9`
  能杀）。修复三层：(1) 独立 `std::thread` 看门狗轮询 fd 0 的 `POLLHUP`（实测 master 关闭后
  立即触发、只订阅 `POLLHUP` 即够），命中即清理退出；shutdown 已触发超 10s 宽限也强制退出，
  让 `kill` 真正有效。(2) 收口**不能**用 `shutdown::terminal_restore_sync()`——它抢 Rust stdio
  全局互斥锁，主线程卡死时永不释放，守卫线程会一起卡死（日志已打「强制退出」但进程仍活着），
  改用 `libc::write(2)` 直写 fd 1/2 的 `force_terminal_restore()`，并加 3s 无条件退出的保险丝
  线程。(3) 全部 4 个读事件入口（`read_line_inner` 主循环 / 快速输入排空 / `drain_paste_burst` /
  `engine.rs::read_key` 即 `/provider *` 子屏）补「进 `event::read()` 前先做 0 超时挂断探测」。
  `isatty(0)` 门控，非 TTY（管道 / CI / e2e）一律跳过，零误杀（实测健康 TUI 连续 45s 不被杀）。
  同轮顺带修：两条提前 `return Err` 路径跳过 `terminal_restore()`（关掉后终端滚动区/底部面板残留）、
  TUI 退出不调 `BrowserManager::shutdown()`（Windows 无 atexit 又无 Drop，Chrome 留后台）、
  `macos_legacy` 的 `open().spawn()` 泄漏 zombie、`human_ui` payload 临时文件改 RAII 清理。
  详见 `docs/TUI输入处理常见陷阱与修复记录.md` §N 与
  `tmpPlan/2026-10-09_终端消失后进程空转不退根治.md`。
- **浏览器可见模式统一与窗口收边（第 139 轮）**：实测「Terminal 里不弹 Chrome 窗口、VS Code 终端里弹」的根因**不是终端差异**——`mcp_web_use::run_open` 把缺省模式**硬编码为 `Hidden`**，且 `BrowserMode::from_env_or_default()`（`LAEW_BROWSER_MODE`/`LAEW_BROWSER_HEADLESS` 的唯一读入口）在整条工具链上**零调用**（只有单测调），于是「是否弹窗」完全取决于 LLM 这一轮有没有自己传 `mode=headed`；两次实测恰好一次传了一次没传，看起来像环境差异。根治：模式决策收敛到唯一真源 `src/agent/browser_mode.rs`，**缺省 = 可见 `headed`**，无 GUI 会话（`CI`/`SSH_*`/Linux 无 `DISPLAY`·`WAYLAND_DISPLAY`/macOS `launchctl managername` 非 `Aqua`）自动回退 `hidden`（保 CI/容器不挂死），环境变量真正接线。窗口尺寸：原实现无条件 `--window-size=1920,1080`，**不按屏幕裁剪**（1440×900/1512×982 屏上右侧底部被挤出屏外 = 用户看到的「显示不全」）也**不区分 chrome 高度**（`--window-size` 设的是外框，headed 下标签栏+地址栏+书签栏吃掉 120~145px，页面真实视口只剩 ~935）。第 139 轮新增 `headed_window_plan` 纯函数：导航完成后实测 `window.screen.avail*`（工作区，已扣 Dock）+ `outer*-inner*`（chrome 高度）——**不自建 macOS `NSScreen`/Windows SPI/`xrandr` 三套原生 FFI**，浏览器自己算的值天生跨平台且随 DPR 正确换算——按「窗口完整落在屏内 + 视口 ≥720p」收边，回 `data.headed_window`。**顺序铁律**：`fit_headed_window` 必须**早于** `fit_viewport_to_content`，因为 `setWindowBounds` 会清除 `Emulation.setDeviceMetricsOverride`，晚一步会把刚扩好的视口覆盖抹掉。设计见 `docs/MCP_Web_Use/01-设计与解决方案.md` §19。
- **可视化模式页面蒙层与人工操作拦截（第 141 轮）**：headed 浏览器中人工与 Agent 的**交叉操作**是实测风险（人工点击改掉页面状态、误触支付/删除、抢占焦点让 `key_press` 打进错误元素），蓝色边框只是「标识」不是「锁」。本轮补「锁」，**双层设计**（唯一最新版 `docs/MCP_Web_Use/05-可视化模式蒙层与人工操作拦截.md`，实现 `src/agent/browser_overlay.rs`）：(L1) CDP `Input.setIgnoreInputEvents(true)` 浏览器级丢弃该页面全部真实用户输入（鼠标/滚轮/键盘/触摸，target 级全 frame、跨导航持久）；(L2) JS 半透明蒙层 + 左下角「🔒 页面已锁定(仅观看)」提示条（`pointer-events:none` 纯视觉，`data-laew-agent="1"` 提取过滤，`addScriptToEvaluateOnNewDocument` 导航自动重注入）。**为什么不用 DOM 蒙层拦输入**：CDP 合成输入与真实输入在 hit-test 层不可区分（`isTrusted` 均为 true），`pointer-events:auto` 会把 Agent 自己的点击一并吃掉；且「挂起标志」跨域 iframe 读不到，Agent 点跨域 iframe 区域会被误拦且无法解除。**实测铁律**：`setIgnoreInputEvents` **同时拦截 CDP 合成输入**（本机探针三实验），因此 20 个输入类 control_action 由 `mcp_web_use::control::run` 统一「**先解后锁**」（动作前 `ignore=false`，动作后 `ignore=true`，毫秒级窗口；蒙层未激活零开销）；导航类动作收尾 re-assert 双层收敛；截图路径（screenshot/ocr/HITL 附图采集）「拍前隐藏蒙层 → 拍完恢复」保证证据链干净；`request_human` 新参 `unlock_page`（默认 true：提问期间自动解锁页面供人工拖滑块/扫码/填表，应答/超时/取消/不可用四种结局全部自动复锁）。工具面：`open(overlay?)`（默认 `LAEW_WEB_OVERLAY`）、`control(set_overlay)`（第 40 个写操作）。边界如实记录：浏览器 chrome（地址栏/标签栏/窗口拖动）不受页面级拦截，彻底物理隔离需窗口级私有 API 方案（脆弱且会废掉人工查看），不采纳；人工自己新开的标签页无蒙层（视为人工自己的页面）。`page_state` window 全局键探测排除 `__laew*`（顺带修掉高亮脚本既有污染）。
- **页面管控三档（第 143 轮）**：把第 141 轮「蒙层开/关」布尔量升级为 **guard 三档枚举**（唯一最新版 `docs/MCP_Web_Use/07-页面管控三档模式与人工交互设计.md`，实现仍集中在 `src/agent/browser_overlay.rs`，与浏览器可见性 BrowserMode、人工介入 HITL 三维正交）：`locked` 屏蔽模式（缺省，行为与第 141 轮蒙层完全一致——CDP `Input.setIgnoreInputEvents` + 半透明蒙层，20 个输入类动作「先解后锁」）；`open` 非屏蔽模式（新晋一等模式：CDP 放行 + 左下角常驻「🔓 页面开放 · 人工可直接操作」状态条，用于人工亲自操作与**人工登录先行流**）；`partial` 部分屏蔽模式（全新：CDP 放行 + **DOM 盾区**拦人工——黑名单 `block_selectors` 每命中元素一个红调盾罩、白名单 `allow_selectors` 用「垂直条带分解」求补集拼出整屏罩+洞（**实测 `clip-path: path(evenodd,…)` 被本机 Chrome 拒绝，不采纳**），Agent 输入动作「先隐盾后复盾」与先解后锁同构）。工具面：`open(guard?/allow_selectors?/block_selectors?/guard_note?)` + `control(set_guard)`（第 41 个写操作；切 locked/open 自动清遗留选择器）+ legacy `overlay`/`set_overlay` 完整兼容；`request_human(unlock_page=true)` 三档通用（第 144 轮：优先 partial 白名单挖洞只放行凭证输入区、收口按期望态恢复，见下条）；截图/OCR/HITL 附图「挂起视觉」避让蒙层+盾区+状态条。状态条为三档共用独立元素（`__laew_overlay_hint__`），文案随档切换、`guard_note`（≤60 字符）为第三行自定义引导。Agent 自主判定注入系统提示词 §17（决策表：缺省/高风险 → locked；「我自己操作/我先登录」→ open；分区协作 → partial）+ §20（通道 B 登录先行流：open(guard=open,guard_note) → request_human(login) → 应答 → set_guard(locked) 收口）。
- **浏览器进程保护与 HITL 凭证区域部分放行（第 144 轮）**：两项根治（唯一最新版 `docs/MCP_Web_Use/08-浏览器进程保护与凭证区域部分放行.md`）。① **绝不关闭用户已打开的 Chrome**——删除第 142 轮 `auto_relaunch` 的「按进程名退出用户浏览器」步骤（`pkill -x`/`taskkill /IM /F` 会杀掉用户全部 Chrome 窗口），改为「运行中 best-effort 复制登录态 → 独立调试 profile 另启 Chrome 实例（多实例靠不同 `--user-data-dir` 并存）→ 接管」；代码库不再存在任何按进程名杀浏览器的路径，laew 只对自己 launch 的浏览器进程（一次性 `laew_browser_*` profile）拥有生杀权。② **HITL 凭证区域部分放行**——`request_human(unlock_page=true)`（默认）提问期间优先切 **partial + allow_selectors 白名单挖洞**：`params.allow_selectors` 显式 > 自动探测凭证输入区（密码框→form/语义容器、验证码/滑块 iframe 与容器、短信/OTP 输入框，`mcp_web_use/unlock_zone.rs` 一次 eval_js 探测），探测不到才回退整页 open（第 143 轮行为）；提问期间主 frame 导航自动重放放行态（登录→2FA 多步流程不断链），收口（应答/超时/取消/不可用）回收重放协程并按期望态恢复；响应附 `data.page_unlock{applied,mode,allow_selectors,source}` 对账。配套：盾区 JS 白名单在**子 frame 零命中时不再整 frame 加盾**（黑名单行为不变）——跨域验证码 iframe 内部必须放行，否则主文档挖好的洞从内部被锁死；`inspect(info=blockers)` 命中阻断时附 `data.credential_zones` 选择器供 LLM 直传 request_human；杀开关 `LAEW_WEB_HITL_SCOPE=off` 回退整页 open。
- **任务锚点（TargetAnchor，第 128 轮）**：从用户原文机械抽取的目标硬约束（不经 LLM，不可幻觉、不可丢失），全局单例（`src/agent/safety/target_anchor.rs`，与 `BrowserManager::global()` / `HumanAssistHub::global()` 同构；进程内 `tokio::task_local!` 同样可以但本轮选全局槽）。跨五层设防：(L0) 输入保真粘贴窗口加宽 + `prompt_lines/prompt_chars` 提交留痕；(L1) 编排器澄清门 `OrchestrationOutcome::DirectAnswer`，Yolo 填 `target_status="unresolved"` 或机械通道命中「指代 + 零主机」时触发；(L2) Main-Work 提示词第六条约束 + `plan_validate.rs` 澄清单元阻断（伪门在 DAG 里无法暂停等待用户，拆了就被秒级打回）；(L3) `MCP_Web_Use action=open/navigate/new_tab` 显式 URL 动作越界返回 `code=6001`，新开 6xxx「范围约束」段，避开 5xxx（MCP_Use）和 4xxx（HITL）的语义冲突；存活浏览器页面提示按 anchor 二分渲染为「✓ 可复用 / ⚠ 禁止复用」组，切断上一轮失败页面被当本轮权威上下文的漂移洗白通道；(L4) QC 目标一致性硬门（与第 109 轮桌面目标保真同形态）+ `target_drift` 强信号（命中即 `is_failed()`）。**第 135 轮收窄**：`detect_target_drift` 不再对整段 `args_json` 跑全文主机扫描，改为解析参数 JSON 后**只取显式导航位 URL**（`open` 顶层 `url`；`control` 里 `navigate`/`new_tab`/`download` 的 `params.url`；`sequence`/`batch` 的 `steps[].params.url`），这些字段只走 scheme 通道，且**跳过 `ok=false`**（被 6001 拦下的尝试不重复计强信号）；裸主机通道同时拒识「标识符续接」片段（紧跟 `-`/`_`/`.` 说明是 CSS 类名 / JS 标识符而非域名）。根因：实测 `div.kr-loading-more-button` 这个 CSS 类名被截成主机 `div.kr`，产出 `target_drift` **误报**强信号，经 `is_failed()` 把 QC 的 pass 强制降级为 Fail，合法单元不可恢复。决策审计新增第 6 决策点 `target_anchor`，阶段 `extract` / `clarify`。实测（`-p` 单轮）：事故精确复现输入下 `outcome="clarification_needed"`、27 秒、零 MCP_Web_Use 调用、零 WorkFlow 单元；对照事故 365 秒、4 个 wf 全在 `ithome.com`、记为「✅ 成功」。开关 `LAEW_TARGET_ANCHOR=off` 全关回退，`LAEW_TARGET_ANCHOR_BLOCK=off` 仅关 L3 阻断保留观察。
- **复用已登录浏览器（connect 模式增强，第 142 轮；第 144 轮进程保护）**：用户在本机 Chrome 已登录某网站时，`open(reuse_existing=true)` 自动探测本机 CDP 调试端口（默认 9222，`LAEW_CHROME_DEBUG_PORT` 可覆盖；依次尝试 `localhost`/`[::1]`/`127.0.0.1` —— Chrome 154+ 的 DevTools HTTP 端点只服务 IPv6 loopback 连接，IPv4 返回 404）并 `Browser::connect()` 接管，保留 Cookie/登录态；探测失败返回 **3002** + `data.relaunch_command`（平台相关重启命令，转述用户执行或 `auto_relaunch=true` 重试，由工具自动「复制登录态关键文件到独立调试 profile → 以调试参数**另启独立 Chrome 实例**（与用户浏览器并存）→ 接管」；**第 144 轮起绝不退出用户 Chrome**，响应附 `login_state_copied/skipped` 复制计数）。关键约束：Chrome 136+ 默认 profile 上调试端口被忽略，必须独立 `--user-data-dir`（故 auto_relaunch 复制 Cookies/Local Storage/Session Storage 等，用户 Chrome 运行中 best-effort 直读）。connect 模式语义：close 只断连不关用户浏览器、页面管控恒为 open 且零注入（不锁用户输入、不向用户页面注入视觉）、`mode` 固定 headed、响应带 `connect_mode:true`。Linux 服务器无登录态可复用（内存无头实例），登录走 `request_human`。实现 `src/agent/browser_reuse.rs`，设计见 `docs/MCP_Web_Use/06-复用已登录浏览器会话.md`、`docs/MCP_Web_Use/08-浏览器进程保护与凭证区域部分放行.md`。


### 多 Agent 架构（8 角色）

| 角色 | 身份 | 职责 | 工具面 |
|------|------|------|--------|
| **Yolo Agent** | `LsmAgentEmergentWork-Yolo` | 入口层：每条输入做 目的→目标→意图 三步分析；任务**三档分类**（simple/medium/hard）；失败回流与用户建议；分类前按 ReAct 自主收集信息（避免 Google 类目的）；延迟强制 `submit_task_classification`（探索轮不注入 forced `tool_choice`、仅末轮强制收口）；**目标可解析性判定**：第 128 轮填 `target_status`（`explicit`/`resolved`/`unresolved`），目标不可解析时编排器直接回问用户（澄清门）不进 WorkFlow，根除「目不明确就猜一个站点」的乱跑行为 | `Read`/`Glob`/`Grep`/`Bash`（只读侦察）/ `MCP_Web_Use`（观察类 action）/ `SubAgent`（只读并行子 Agent） |
| **Plan Agent** | `LsmAgentEmergentWork-Plan` | 规划层：仅在 hard 任务时启用；输出 Markdown 方案到 `plans/{session_id}-{seq}.md` | `Read`/`Write` |
| **Main-Work Agent** | `LsmAgentEmergentWork-Main-Work` | 流程层：接收 medium/hard 任务，拆 WorkFlow 列表（Kahn 分层 + 同层并行）；编排循环 ReAct 化（TaskFocus → Verify+Decompose → Emit 三段式 + 任务前提验证硬性要求）；复用 LoopGuard 编排层原地打转同样止损；迭代预算 `max_iterations(8)` + `explore_budget(2)` | `Bash`/`Read`/`Glob`/`Grep`/`MCP_Web_Use`（编排前探查）/ `TodoWrite`/`SubAgent`（**不持** `Write`/`Edit`，流程层只编排不落源代码；不持 `MCP_Window_Use`，桌面窗口操控归 SubAgent-Work 专用） |
| **SubAgent-Work Agent** | `LsmAgentEmergentWork-SubAgent-Work` | 执行层最小单元，每个流程处理单元委派一个 SubAgent；提示词 ReAct 化（Thought→Action→Observation）+ 工具连续工作模式（同一响应里连续的 `parallel_safe` 工具 Read/Glob/Grep 用 `join_all` + `Semaphore(4)` 并发执行 + `tool_result` 保序回填）+ 无进展止损（`agent/loop_guard.rs` 进展键 = 工具名 + 参数稳定 JSON + 结果摘要，轮询等待类合法重复零误伤；`wait_like()` 让 wait/纯 sleep/`SubAgent(result|history)` 透明跳过；`NUDGE_AT=2`/`ABORT_AT=3` 双阈值 + 宽限轮强提醒作为 user 消息延迟到 tool_result 回填完再推入避免破坏 Anthropic 400 配对）；runtime hints 角色化（`HintRole{Ui,Execute,Gather,Judge}` 由实际调用过什么工具决定） | `Bash`/`Read`/`Write`/`Edit`/`Glob`/`Grep` + 平台门控注入 `MCP_Window_Use`（仅 macOS/Windows）+ `MCP_Web_Use` + `MCP_Use`（通用 MCP）+ `TodoWrite` |
| **Quality-Check Agent** | `LsmAgentEmergentWork-Quality-Check` | 质检层：每个执行单元完成后必经 QC；QC LLM 错误与用户取消不消耗单元 retry 预算；**目标一致性硬门**（第 128 轮）：用户指定 X、实际操作 Y（含改派到无关站点）一律判 Fail + `retryable=false`，retr y 不会让错位目标变成正确目标 | 可选 `Read` |
| **SessionContext Agent** | `LsmAgentEmergentWork-SessionContext` | 会话层：每个用户任务完成后汇总并写入 `session_memory` 表；Yolo 下次处理时自动注入最近 N 条（默认 3）历史摘要 | 无工具 |
| **Debug Agent** | `LsmAgentEmergentWork-Debug` | 调试层：仅在 `-debug` 调试模式下启用；任务结束后对采集的 trace（各 Agent LLM 调用输入输出 / Yolo 分类 / QC 结论 / 耗时与 token / 错误）做评估，产出「任务评估 / 质量报告 / 问题报告(P0-P2) / 优化建议」四章节；报告写入**根目录** `DebugReport/debug_report_{YYYYMMDD}_{HHMMSS}_{随机6位}.md`（已 gitignore，不入库） | 无工具 |
| **Compact Agent** | `LsmAgentEmergentWork-Compact` | 压缩层：Session 主上下文估算 token（字符/4 +10%）达到当前 Provider `context_max_size` 的 80% 时由 Orchestrator 自动触发，按超出幅度自动选三档压缩率（Light ≤80% / Medium ≈50% / Aggressive ≤20%），LLM 摘要失败降级本地硬截断；保护带项目上下文/历史摘要/已压缩标记的消息与最近 4 条消息；**溢出兜底（reactive）**：真实溢出（Provider 返回 `prompt is too long` / `context_length_exceeded` 类 400）时由 Agent 循环自动三级恢复——排水（截短超长 tool_result）→ 折叠（历史合并为压缩摘要）→ 暴露（原错误上抛），见 `agent/overflow.rs`（一处包裹、8 角色全生效，全会话恢复预算 4 次） | 无工具 |

#### 跨角色编排（`MultiAgentOrchestrator`）

用户输入 → 项目上下文注入 → Yolo 分类 → 简单档（SubAgent）/ 中档（Main→SubAgent）/ 高档（Plan→Main→SubAgent）→ Quality-Check → SessionContext 收口。

- WorkFlow 执行时按 `depends_on` 自动 Kahn 分层（`main_work::topo_layers`），**同层无依赖的 SubAgent 自动并行**（tokio::spawn + Semaphore 上限 3，`OrchestratorConfig::max_parallel_workflows`），跨层严格串行、上游产物按层注入，失败语义与串行一致（fail-fast 回流 Yolo）
- **执行-验证-修订闭环**：WorkFlow 单元 QC 判 `retryable=true` 时先在**单元级局部重试**（仅该单元，注入本单元 QC 结论，不连坐同层姊妹单元），`OrchestratorConfig::unit_retry_budget` 默认 2（单单元最多 3 次尝试，`0` = 关闭旧行为）；预算耗尽才升级到档位级重试（`max_retry_per_level=3`，`retray_hint` 回灌 Main-Work）→ Yolo 回流（`[PREVIOUS_FAILURE]` + `failure_signals`）→ Failed outcome。retry_hint 分层：attempt=0 用档位级 hint / attempt≥1 用本单元 QC issues+suggestion 覆盖 description hint 段（`apply_retry_hint_overlay`）

### 三个 MCP 风格工具（替代已删除的独立 Agent 角色）

| 工具 | 替代 | 能力 | 平台门控 | 设计文档 |
|------|------|------|----------|----------|
| **MCP_Window_Use** | 原 WindowUse Agent | 桌面窗口操控：单工具 `action` 枚举分发（`open`/`list`/`find`/`inspect`/`control`/`ocr`/`screenshot`）；**双路线**（Windows UIA Pattern 优先 + Win32 消息 + 物理鼠标键盘兜底；macOS AX 控件树 + 物理输入；Linux wmctrl/xdotool 物理层）；操作优先级链 T1 UIA Pattern → T2 Win32 消息 → T3 物理输入（返回 `route=uia/win32_msg/physical` 标注供 QC/Debug 对账）；修饰键+鼠标/中键同时操作；复合 `input_batch` 一次编排 ≤40 步；视口/OCR 自适应 | **仅 macOS/Windows** 运行时注册进 `builtin_registry()`；Linux 走物理层兜底 | `docs/MCP_Window_Use/01-设计与解决方案.md` |
| **MCP_Web_Use** | 原 Chromium-WebUse Agent | 浏览器操控：单工具 `action` 枚举分发（`open`/`list`/`close`/`control`/`inspect`/`sequence`/`batch`/`explore`）；`control_action` 41 个写操作（鼠标/键盘/拖拽/上传/下载/eval_js/Cookie/视口/截图/蒙层开关/页面管控切换等）；`inspect` **19** 个观察维度（含 Console/Network/Elements/DOM/localStorage/Cookie/页面元信息/OCR/`blockers` 人工阻断检测/**`extract` 结构化列表抽取**（第 135 轮：一次调用把列表页压成结构化条目并可在**页面内**完成关键词过滤 + 时间窗 + 排序 + 截断，只回精简字段；`probe=true` 先侦查候选选择器免去盲试；时间支持中文相对时间「3小时前」并按**页面本地时区**解析）/ **`page_state` SSR 注水数据直读**（第 135 轮：枚举 `window.__NEXT_DATA__`/`initialState` 等注水点 + 探测 window 全局键 + JSON-LD，三闸裁剪并以 `dropped_paths` 显式告知被裁部分））；单步 + 连续（`sequence` ≤24 steps，批内 `$page_id`/`$spawned_page_id` 占位自动跟随派生新页）；视口基准 1080p + 2K 自动扩展（Playwright/Puppeteer 同款机制，CDP 坐标恒 CSS 像素无 DPR 换算）；验证码 OCR（macOS Vision）+ eval_js 容错与结果净化（自动 IIFE 重试 + 匿名 `function(){}` 自动包裹 + 大结果/data-url 落盘 + **第 135 轮复合值体积闸门**：数组/对象序列化超 20KB 先就地裁剪再落盘，此前只有字符串走 2000 字符闸门、复合值零上限，实测出现过 17~19KB 大对象直灌）+ CDP 下载管理（`data:` 直存/`about:/blob:/javascript:` 快速失败）；**第 139 轮默认可见模式**（缺省 `mode=headed`，`LAEW_BROWSER_MODE` 真正接线，无 GUI 会话自动回退无头）+ **窗口按屏幕工作区自动收边**（小屏笔记本不再把 1920 窗口挤出屏外「显示不全」，页面视口下限 720p、默认 1080p，用 `screen.avail*` 与 `outer-inner` 实测而非自建跨平台 FFI）+ 蓝色选中边框与「LAEW Agent 控制中」徽标 + **第 141 轮可视化蒙层与人工操作拦截**（headed 页面半透明蒙层 + CDP 输入锁定：人工可实时观看不可点击，防交叉操作；Agent 输入动作自动「先解后锁」，截图自动避让蒙层，`request_human` 默认提问期间解锁/收口复锁，`LAEW_WEB_OVERLAY=off` 全关回退）+ **第 143 轮页面管控三档**（蒙层升级为 guard 枚举：locked 屏蔽（缺省）/ open 非屏蔽（人工可直接操作+状态条明示，人工登录先行流）/ partial 部分屏蔽（DOM 盾区：黑名单红盾罩 / 白名单条带分解挖洞，Agent 动作「先隐盾后复盾」）；`open(guard/allow_selectors/block_selectors/guard_note)` + `control(set_guard)` 运行时切换 + Agent 按用户提示词自主选档，设计 `docs/MCP_Web_Use/07`）；**人工介入 HITL**（`src/agent/human_assist.rs` 全局枢纽，滑块/短信/扫码登录等不可自动跳过流程结构化提问，人工答复经 oneshot 回填；**呈现端双通道第 130 轮**：macOS/Windows 桌面**弹窗 UI 优先**（`laew __hitl-dialog` 原生子进程：macOS AppKit FFI / Windows Win32，第 137 轮重构，持续置顶+倒计时+时间轴+验证码图片+最大化/查看原图，`-p` 模式同样可弹），TUI 兜底行读，弹窗失败自动降级；二者均不可用 fail-fast 4001/4002）+ **第 142 轮复用已登录浏览器**（`open(reuse_existing=true)` 自动探测本机调试端口并接管用户已登录 Chrome 保留登录态；探测失败返回 3002 + 平台相关重启命令，`auto_relaunch=true` 自动「复制登录态 → 独立调试 profile 另启实例 → 接管」；connect 模式 close 只断连不关用户浏览器、默认不锁人工输入）+ **第 144 轮浏览器进程保护与凭证区域部分放行**（全链路**绝不关闭用户已打开的 Chrome**，auto_relaunch 复制登录态 best-effort + 独立实例并存；`request_human` 提问期间账号/密码/验证码区域自动成为 partial 非屏蔽区域（白名单挖洞），探测不到才整页 open，提问期间导航自动重放，收口恢复原档；盾区白名单子 frame 零命中不再加盾（跨域验证码 iframe 可点）；`inspect(blockers)` 附 `credential_zones`；`LAEW_WEB_HITL_SCOPE=off` 杀开关，设计 `docs/MCP_Web_Use/08`） | 跨 Windows/macOS/Linux；Chrome→Edge→Chromium→Brave 自动探测 | `docs/MCP_Web_Use/01-设计与解决方案.md` |
| **MCP_Use** | 新增（通用 MCP 协议调用） | 通用 MCP（Model Context Protocol）服务调用统一入口：真 MCP 协议客户端（JSON-RPC 2.0），连接外部 MCP server（stdio 子进程 / Streamable HTTP）；单工具 `action` 枚举分发（`list_servers`/`connect`/`list_tools`/`call_tool`/`list_resources`/`read_resource`/`close`）；server 接入记录在 SQLite `mcp_servers`（`laew mcp add|list|del|test` 维护，headers 经 Vault 加密），**LLM 不可新增 server**；懒连接 + 指数退避重连稳定性窗口 + `kill_on_drop` 防子进程泄漏；ContentBlock 四类投影降级永不丢弃；统一 JSON 信封 0/1001/3001/5001-5006；注册进 SubAgent-Work + Main-Work（编排探查），Yolo/Plan/QC 不持 | 跨平台（由外部 MCP server 决定能力） | `docs/MCP_Use/01-设计与解决方案.md` |

### 自感知动态子 Agent（Self-Awareness SubAgent）

模型自主委派通道：Agent Tools 新增 `SubAgent` 单工具（`action` 枚举 `launch` / `batch` / `list` / `result` / `cancel` / `history` / `resume` / `workflow`）。
**用户提示词显式要求**（「启动 SubAgent / 并行 / 分工 / 分别调研 / 多个 Agent」）或任务可分解为 2+ 独立子任务时，Agent 自动组建子 Agent 小队并汇总结果。

- **子 Agent 6 类型**（内置）：`general-purpose` / `explore` / `researcher` / `plan` / `code-reviewer` / `operator`
- **自定义类型**：`.laew/agents/*.md` + `~/.laew/agents/*.md` 两级发现（与 D2 自定义斜杠命令同构，共享 `src/frontmatter.rs`）；frontmatter 支持 `label/description/extends/tools/readonly/name`；正文 = 专属职责提示词；定义即生效（不缓存、不改代码不重编译）；内置 6 类 id（含别名）不可被遮蔽
- **工具面三重收窄**：`tools` 白名单 ∩ `extends` 默认 ∩ `SpawnPolicy` 上限 ∩ 真实注册表，被剔除项进 `dropped_tools`
- **运行持久化**：每次运行落 SQLite `subagent_run`（`insert running` → `update finish`，单一入口 `run_child`，fail-open）；`action="history"` 跨任务/跨会话查询（零 LLM 成本）；`action="resume"` 前序任务+结论作为种子上下文注入新子 Agent（消耗一次预算，不是进程级恢复）
- **工作流编排**（`action="workflow"`）：≤8 个 `steps[]` 声明 DAG（每步唯一 `id` + 自包含 `task` + `depends_on`）；Kahn 分层 + 同层并行 + 跨层串行；上游成功结论按受控截断注入下游；上游失败→直接下游 `skipped`、间接下游 `blocked`
- **治理**：默认 1 层嵌套（叶子不注册该工具 + 运行时 2002 双重防御）；会话级 `Semaphore(3)` + 预算 8 + 单子 Agent 12 轮 / 300s；父 `CancelToken` → `child_token()` 级联取消；用量经会话台账计入 `/cost`
- **自感知三层**：① 系统提示词身份段（工具清单由 `ToolRegistry` 程序化生成，与真实工具面不漂移）② `SubAgent(action="list")` 运行时快照（深度/余额/运行中作业）③ 事件环 + TUI `/agents`
- **失败处理**：子 Agent 失败**不**触发 Yolo 失败回流，由父就地补做/改派且必须汇总成最终回答
- **工具返回统一 JSON 信封**：`0/1001/2000/2001/2002/2003/4001/4002/4003`

实现 `src/agent/{self_awareness,dynamic_subagent,custom_agents,subagent_workflow}.rs` + `src/agent/tools/subagent.rs` + `src/config/subagent_run.rs`，设计见 `docs/自感知SubAgent动态启动/01-设计与解决方案.md`。

### Skill 系统（渐进式披露 P0）

- Markdown 文件定义，frontmatter 支持 `name`/`description`/`allowed-tools`/`when-to-use` 等
- 三级加载：元数据始终在 system → 内容按需注入 → 完整文件按需 Read
- 内置 skill：`code-review` / `git-commit` / `test-runner`（位于 `src/agent/skills/bundled/`）
- 用户级 + 项目级两级覆盖（`.laew/skills/*.md` + `~/.laew/skills/*.md`）
- 完整设计见 `docs/Skill系统与渐进式披露/01-设计与解决方案.md`

### Agent-Context / Agent-Memory / SessionContext

- **Agent-Context**：每个 Agent 独立的实时上下文（消息流 + 状态），内存态，生命周期 = 当前单元
- **Agent-Memory**：每个 Agent 独立的记忆层（输入/输出/错误/产物摘要），持久化到 SQLite `agent_memory` 表，跨单元/跨 Session 复用
- **SessionContext 摘要**：每个用户任务完成后 SessionContext 生成 Markdown 摘要写入 `session_memory` 表；Yolo 下次处理时自动注入最近 N 条历史摘要（默认 3），用 `<<<LAEW:SESSION_HISTORY>>>` 标记隔离
- 与 Session 主上下文（用户对话历史）严格隔离

### AgentProfile / Session / 请求头 / 系统提示词三段式

- **AgentProfile**：Agent 身份档案（名称 / 系统提示词 / 工具集 / `spawn_policy`），`work_profile()` / `yolo_profile()` / `dynamic_child()` 工厂函数
- **Session**：进程内会话，拥有独立 Session ID 与对话上下文（context）；TUI 启动或 `/new` `/clear` 时生成新 Session
- **会话持久化**：TUI 每轮任务收口把 transcript **整快照重写**（单事务 DELETE+INSERT）到根目录 SQLite `chat_sessions`（索引行：title/turn_count/model_name/updated_at）+ `chat_turns`（轮次：`response` 人类版与 `context_response` 上下文回填版分列）；失败仅告警不打断对话；自动保留最近 50 个会话。恢复走 `/sessions` `/resume` / `--resume`（`-c`），重建后保持原 Session ID（`session_memory` 摘要链连续）、PROJECT_CONTEXT 幂等重注入
- **请求头**：两协议统一携带 `User-Agent: {AgentName}/{版本} {编译时间}`、`Authorization: Bearer {api_key}`、`X-Session-Id`；Anthropic 请求体 additionally 携带 `metadata.user_id`（含 `device_id/account_uuid/session_id/agent`）。**User-Agent 按"发起请求的 Agent 角色"逐请求注入**（`Agent::run_session_inner` 从 profile 写入 `RequestMeta.user_agent`，8 角色各自携带自身名称，抓包层面可辨识）
- **Anthropic 三段式系统提示词**：8 角色 + WorkFlow 角色的 `system` 字段从单字符串重构为三段式，对齐 Claude Code CLI 抓包范式——
  1. **billing 计费头**（无 `cache_control`）：单行 `x-anthropic-billing-header: cc_version=...; cc_entrypoint=cli; cc_is_subagent=true;`，Anthropic 内部计费/链路字段，对模型行为零影响
  2. **identity 基础身份声明**（带 `cache_control: ephemeral`）：单行 Agent 身份 + 一句话职责，8 角色 + WorkFlow 各一份
  3. **rules 核心行为规则**（带 `cache_control: ephemeral`）：base + tools_hint + protocol_tail + 准静态 workspace_hint，等价于原 `render()` 单字符串内容
  缓存复用：三段全部静态/准静态 → 缓存断点逐轮命中。**runtime hints 不进 system**（第 140 轮）：逐轮变化的进度/收口/无进展提醒经 `RequestMeta.runtime_tail` 由 wire 层拼到消息流末尾尾注（`llm/anthropic.rs::append_runtime_tail` 追加到最后一条 user/tool 消息 content 尾部，不持久化 session context），否则会摧毁 rules 块 cache_control 断点之后的全部缓存前缀（实测 46/172 次 cache_read 仅 188）。4 断点 cap 约束下，billing 无 cache / identity + rules + last tool + latest user 各一份 cache，刚好命中 cap。OpenAI 协议走 `system` 单字符串 + 同一尾注注入位。实现 `src/agent/system_prompt/mod.rs::PromptSegments` + `src/llm/anthropic.rs::convert_system_blocks_split`，Agent 循环 `meta.anthropic_segments = Some(profile.system_prompt.prompt_segments())` 注入。

## 架构（src/）

```
main.rs            clap CLI 入口（默认进 TUI；-p 单轮；-f 文件提示词；provider / mcp 子命令）
lib.rs             库导出
session.rs         Session:本机指纹 device_id + Session ID + 独立对话上下文 context
error.rs           AgentError(含 YoloParse)
shutdown.rs        graceful shutdown(TUI 退出时清浏览器子进程,避免双重 panic)
crash.rs           CrashDump:panic hook + 信号恢复 + heap dump 自动触发
frontmatter.rs     Markdown frontmatter 解析(自定义斜杠命令 + 自定义子 Agent 类型共用)
logging.rs         tracing 双层订阅器:控制台层行为不变 + 文件层运行日志(llaew_*.log)
test_support.rs    单元测试公共夹具

tui/
  mod.rs           会话外壳:TuiSession 结构 + 生命周期(bootstrap/banner/provider 增删查/分支快照)+ orchestrator 装配 + run()/run_with_debug() 入口
  dispatch.rs      输入分发与任务输出:handle_user_input / dispatch_prompt(@提及展开 + 阶段进度协程 + SIGINT 取消)/ emit_debug_report / print_* 家族
  slash.rs         斜杠命令路由:handle_slash + run_theme/run_rewind/run_undo/run_fork/run_branches/run_switch/run_export + print_custom_commands
  provider_screen.rs  /provider 子屏桥接:list/add/del 三屏接入 + run_screen_loop(通用 Screen 栈循环,非 TTY 回退 print)
  format.rs        纯函数格式化:任务结果双版本(人类版 format_task_result / LLM 回填版 for_context)/ merge_usage / waiting_line_text / clip_cols(显示列截断)/ print_help(信息盒)/ print_record
  format_brief.rs  [tool]/[laew] 行关键字段摘要(MCP_Window_Use 焦点守卫/route/verified 等)
  audit_view.rs    /audit 决策审计可视化面板
  banner.rs        启动横幅:声明式行集(BannerData)+ textfit 自适应渲染 / 工作区·连接·日志行文本纯函数
  engine.rs        CLI 渲染引擎 —— Screen trait + Frame + 全量重绘 present
  form.rs          通用 Tab 表单状态机(被 ProviderForm 屏复用)
  hitl_view.rs     人工介入(HITL)TUI 呈现视图(第 130 轮自 dispatch.rs 拆出):请求块渲染/倒计时同行右对齐/行读映射/弹窗接管通知行与事件打印
  input.rs         单行输入(主屏用):行编辑 + 行内提示 + 补全 + **自适应底部面板**(DECSTBM 滚动区;面板顶行紧跟光标,不留白,查不到光标行时回退吸底 —— 第 134 轮)
  paste.rs         粘贴保真层(从 input.rs 拆出):PasteRegistry 登记簿 / handle_paste_text / 粘贴预览与提交完整回显(纯函数)
  completion.rs    斜杠命令补全引擎(内置 + 自定义命令动态注册)
  commands.rs      自定义斜杠命令(D2):两级目录发现/frontmatter/占位符渲染
  export.rs        会话导出(D8):transcript 记录 + Markdown/JSON 落盘
  branches.rs      对话分支存储(D3):rewind/fork/switch/clear 前自动快照(内存态上限 10)
  mention.rs       @ 文件提及解析 + 实时路径补全 + 目录钻取
  pathfmt.rs       路径格式化辅助(相对路径展示/工作目录锚定)
  textfit.rs       显示宽度唯一真源:char_width/width/clip/clip_mid/wrap/pad + InfoBox 自适应信息盒(横幅与 /help 共用)
  theme.rs         ANSI 颜色 / mask_key 脱敏 / attrs·bg·color→ANSI 转换 集中管理
  term_guard.rs    终端守卫(第 136 轮):看门狗线程 + POLLHUP 挂断探测 + 无锁终端还原
  screen/
    provider_list.rs   /provider list —— Tab 化展示 + 操作按钮
    provider_form.rs   /provider add —— 5+1 Tab 表单
    provider_del.rs    /provider del —— Picker + 二次确认

agent/
  mod.rs           agent 域模块注册表 + Agent 结构体/构造器/访问器(协议无关循环的总装配)
  agent_loop.rs    Agent 核心循环:run_session(Session) → complete → tool_calls → 分批并发执行 → tool_result 保序回填 + 截断续接/溢出恢复 + ReAct 守卫接入/收口预告
  agent_message.rs Agent 消息模型(协议中立统一表示)
  attachments.rs   @ 文件提及附件注入(<<<LAEW:ATTACHMENTS>>>)
  tool_exec.rs     工具调用执行底座:exec_tool_call 单条执行归一(Schema 预校验/取消竞争/错误归一) + plan_parallel_batches 连续 safe 段切批 + run_parallel_batches(join_all + Semaphore 限流)
  loop_guard.rs    ReAct 循环守卫:无进展检测(doom_loop,进展键含 Observation 摘要) + NUDGE_AT=2/ABORT_AT=3 双阈值 + 宽限轮 + wait_like 等待类透明化
  runtime_hints.rs Agent 循环运行时辅助:runtime hint 拼装(HintRole 角色化 + ReAct 进度/Thought/收口/无进展四类 hint)/截断判定/首迭代强制工具开关/稳定 JSON 序列化
  context.rs       Agent-Context 独立实时上下文
  extrace.rs       提取/抽取工具输出关键信息的助手
  cancel.rs        取消竞争:跨回合 CancelToken
  react_tests.rs   ReAct 行为单测

  profile.rs       AgentProfile(名称 / 系统提示词 / 工具集 / spawn_policy) + work_profile()/yolo_profile()/dynamic_child() + User-Agent
  custom_agents.rs 自定义子 Agent 类型(D115):两级发现(.laew/agents)/frontmatter->AgentDef/ResolvedAgentType/名册可见性
  self_awareness.rs 自感知层(D114):6 环境变量配置 / 6 类子 Agent 名册 / SpawnPolicy 三档 / 静态身份段渲染(工具清单由注册表生成)
  dynamic_subagent.rs 动态子 Agent 运行时(D114):会话级 Governor(Semaphore/预算/台账/作业表/事件环) + tokio task_local 作用域 + 子 Agent 组装/运行/回收 + drain_usage
  subagent.rs      SubAgent 主逻辑(D114):launch/batch/list/result/cancel/history/resume 单工具入口
  subagent_workflow.rs SubAgent 工作流编排(D116):≤8 步 DAG + Kahn 分层 + 原子预扣预算
  decision_audit.rs 决策审计(D9-8):5 决策点 → AuditTrail/*.jsonl 三段式结构化记录
  human_assist.rs  人工介入 HITL 枢纽(D100/第 130 轮):pending 槽位 + oneshot + kind 标签/默认文案单一事实源 + 弹窗呈现接线(via=gui/mark_gui_failed/事件环);弹窗与 TUI 均不可用才 fail-fast
  human_ui/        人工介入弹窗 UI 呈现层(第 130 轮;第 137 轮重构为原生子进程):mod.rs(门面/能力探测/payload/结果解析/子进程管理)+ dialog_main.rs(__hitl-dialog 子进程入口)+ macos_dialog.rs(AppKit FFI 原生弹窗:objc runtime/控制器 IMP/布局纯函数/定时器)+ windows_dialog.rs(Win32 原生弹窗:窗口类/控件/WM_SIZE 全量重排/BGRA 位图)+ tests.rs
  todo_state.rs    TODO 任务状态(D19):`/tasks` 数据底座
  memory.rs        Agent-Memory SQLite 持久化
  max_tokens_state.rs max_tokens 三级恢复状态机
  project_context.rs 项目说明文件五级链发现 + README 自动生成 + 每会话首次注入(幂等标记)
  session_context.rs SessionContext Agent
  session_fork.rs  对话 Rewind 轮次扫描(D3):合成消息识别 + 截断边界(供 /rewind /undo /fork /switch)
  workspace.rs     工作区感知(D4):懒刷新快照(git 分支/变更计数/工程类型与工具链建议/顶层结构/6h 最近改动)+ TTL 缓存 + 8 角色 system brief `<<<LAEW:WORKSPACE>>>` + 会话级「工作区快照」段 + TUI 变更对比
  offline_queue.rs 离线模式(D13):请求队列 + 本地缓存 + 同步合并

  json_repair.rs   JSON 自动修复链
  partial_json.rs  partial JSON 解析(应对 Anthropic tool_use 流式截断)
  overflow.rs      上下文溢出检测(15+ provider 正则) + 三级恢复(排水/折叠/暴露)
  plan.rs          Plan Agent
  plan_validate.rs Plan 输出校验
  yolo.rs          YoloRunner 双 Agent 编排器 + TaskLevel + TaskClassification + JSON 解析
  main_work/       Main-Work 流程层目录:mod.rs(MainWorkRunner) / spec.rs(WorkFlow 规格模型+宽松反序列化) / delegate.rs(委派推断 GUI 优先) / topo.rs(Kahn 分层+依赖治理) / parse.rs(JSON/Markdown 双通道解析) / tests.rs
  orchestrator/    MultiAgentOrchestrator 总编排器目录:mod.rs(结构体+入口+进度通道) / types.rs(共享类型) / pipeline.rs(handle_inner+三档链路) / workflows.rs(分层并行+run_wf_unit) / yolo_reflow.rs(Yolo 分类封装+失败回流) / usage.rs(用量累加) / tests.rs

  quality.rs       Quality-Check Agent + 单元 QC 提示词构建 + retry 预算
  debug.rs         Debug Agent(-debug 模式):trace 评估 + 四章节报告生成
  compact.rs       CompactRunner:token 估算 / 三档选档 / 自动压缩触发 / 硬截断降级 / 保护段识别
  subagent_workflow.rs SubAgent 工作流编排(同上)

  permissions/     权限管控:mod.rs / dangerous.rs / readonly.rs / sensitive.rs
  safety/          安全防护:mod.rs / url_safety.rs(SSRF 拦截) / prompt_injection.rs(提示注入检测) / credentials.rs(凭证脱敏) / target_anchor.rs(任务锚点,跨 5 层设防的唯一事实源)
  sandbox_hook/    沙箱钩子:mod.rs(单文件,接外部 sandbox)
  skills/          Skill 系统(渐进式披露):mod.rs / registry.rs / render.rs / tools.rs / skill.rs / bundled.rs / bundled/{code-review,git-commit,test-runner}.md

  system_prompt/   SystemPrompt 组合与渲染:mod.rs / mcp_use_hint.rs / web_evidence.rs(网页取证纪律第 131 轮) / web_extract.rs(网页内容提取纪律第 135 轮) / skill_catalog.rs
  tools/
    mod.rs         Tool trait + ToolRegistry(有序) + builtin_registry()/yolo_registry() + max_parallel_tools_from
    bash.rs        BashTool(UTF-8 环境注入 + CRLF 感知 + 30K/150K 截断 + 落盘回灌)
    bash_spill.rs  BashTool 大对象溢出(D17):落盘后回灌路径
    read.rs        ReadTool
    write.rs       WriteTool
    edit.rs        EditTool(old_string 唯一性 + 4 级匹配 ladder)
    glob.rs        GlobTool
    grep.rs        GrepTool
    emit.rs        EmitTool(合成消息 / 锚点)
    todo.rs        TodoWrite(TODO 任务清单 D19)
    subagent.rs    SubAgentTool(D114 自感知委派:launch/batch/list/result/cancel/history/resume/workflow + 统一 JSON 信封)
    read_detect.rs ReadTool 二进制/编码自动探测

    mcp_window_use/  MCP_Window_Use 工具目录(仅 macOS/Windows 注册):mod.rs(门面+共享辅助) / query.rs(action=list/find+模糊打分) / open.rs(action=open+应用启动) / inspect.rs(action=inspect/control) / vision.rs(action=ocr/screenshot) / chat.rs(微信/豆包等 IM chat_send + read_text + 焦点守卫) / explore.rs(action=explore 树预算剪枝) / sequence.rs(run_sequence assert_text/wait_for_text OCR 兜底) / input_batch.rs(复合 action ≤40 步) / tests.rs
    mcp_web_use/     MCP_Web_Use 工具目录:mod.rs(门面+八 action 分发+视口自动扩展接线+guard 参数级联) / control.rs(40 个写操作,第 143 轮拆出管控子模块) / guard_ctl.rs(页面管控 control_action:让路辅助「先解后锁/先隐盾后复盾」+ set_overlay legacy + set_guard 三档切换,第 143 轮) / inspect.rs(19 个只读观察分发) / eval_sanitize.rs(eval_js 与 data-url 返回值净化 + 复合值体积闸门,第 135 轮) / extract.rs(inspect(info=extract) 结构化列表抽取 + 页面内过滤 + probe 选择器侦查,第 135 轮) / page_state.rs(inspect(info=page_state) SSR 注水数据 + JSON-LD 直读 + 三闸裁剪,第 135 轮) / unlock_zone.rs(HITL 凭证区域探测 + 临时放行导航重放 + LAEW_WEB_HITL_SCOPE 杀开关,第 144 轮) / tests.rs
    mcp_use/         MCP_Use 工具目录(通用 MCP 服务调用):mod.rs(门面+七 action 分发+JSON 信封 0/1001/3001/5001-5006) / tests.rs

  window/          窗口操控平台驱动层(MCP_Window_Use 服务实现):mod.rs(模型+WindowDriver trait+工厂) / windows.rs(UIA+Win32) / windows_input.rs / windows_ocr.rs / macos_axui.rs(AX) / macos_vision_ocr.rs(Vision OCR) / control_action.rs / fallback.rs(wmctrl/xdotool) / macos_legacy/(mod.rs FFI+Driver入口 / ax_attrs.rs / cg_event.rs CGEvent 输入底座 / inspect.rs AX 控件树遍历 / act.rs / tests.rs)
  safety/web_evidence.rs  网页取证纪律(第 131 轮):Bash 网络取证命令识别 / 命令行 host 抽取(含裸 IP 字面量)/ 任务锚点比对 / 拒绝文案
  browser.rs       浏览器 CDP 驱动层(MCP_Web_Use 服务实现):BrowserManager 单例(page_id 注册表 + 跨平台浏览器检测 + Console/Network 缓冲 + 下载事件管理 + 生命周期回收) + fit_viewport_to_content 2K 自动扩展 + fit_headed_window 有头窗口收边(第 139 轮;模式与尺寸决策经 pub use 从下列两个子模块再导出)
  browser_mode.rs  浏览器启动模式决策唯一真源(第 139 轮):BrowserMode 三档枚举 + DEFAULT=Headed + from_arg/from_mode_env_value/from_legacy_headless_value/resolve + has_gui_session(CI/SSH/Linux 无 DISPLAY/macOS launchctl 非 Aqua → 无 GUI 自动回退 Hidden)
  browser_overlay.rs 页面管控模块(第 141 轮蒙层,第 143 轮扩展三档):PageGuardMode/PageGuardConfig(校验/env 级联)/管控 JS 模板(蒙层+状态条+盾区三件套,烘焙 {MODE}/{ALLOW}/{BLOCK}/{NOTE};白名单=垂直条带分解求补集)/inject_agent_guard/apply_page_guard/guard_suspend_visuals + legacy apply_page_overlay/mask_set_visible 委托 + action_dispatches_input 白名单 + 单测(含 #[ignore] 真浏览器三档集成验证)
  browser_viewport.rs 窗口与视口尺寸决策(第 125 轮起从 browser.rs 机械搬移,第 139 轮扩展):DEFAULT_WINDOW_*(1920×1080)/VIEWPORT_FIT_MAX_*(2K)/MIN_VIEWPORT_*(1280×720 视口下限)/HEADED_SCREEN_MARGIN + default_window_size/viewport_fit_plan(只放大)/headed_window_plan(按 screen.avail* 收边 + chrome 补偿)/测量 JS + 单测
  browser_watchdog.rs Browser 子进程 watchdog:TUI 退出时清理 + 防止 panic-in-panic
  workflow/        Goal 状态机 + Squad 调度工作流层:mod.rs / adaptive_loop.rs / batch.rs / goal.rs / phase.rs / quality_gate.rs / squad.rs / template.rs
  workflow_json_validate.rs Workflow JSON 校验

llm/
  mod.rs           统一消息模型 + LlmClient trait + RequestMeta + build_common_headers + build_http_client(TLS 三级策略:IP 主机自动放宽自签名证书 / LAEW_TLS_INSECURE 全局开关)
  anthropic.rs     Anthropic wire 转换(x-api-key + anthropic-version + metadata.user_id + 三段式 system blocks)
  openai.rs        OpenAI wire 转换(Bearer)
  sse.rs           SSE 流式响应解析
  cancellable.rs   可取消 LLM 调用(CancelToken)
  resilient.rs     LLM 调用自动弹性层(指数退避 + 熔断 + 错误重试)
  cache_policy.rs  PromptCaching 策略(cache_control 断点 + 4-chars/token 启发式)
  offline.rs       离线模式 LLM 调用降级
  pricing.rs       Token 单价表(用于 /cost)

mcp/               通用 MCP 客户端层(MCP_Use 服务实现):mod.rs(类型+ContentBlock 投影+错误码映射) / jsonrpc.rs(JSON-RPC 2.0 帧) / transport.rs(stdio NDJSON + Streamable HTTP) / client.rs(握手/tools/list 分页折叠/tools/call/resources) / manager.rs(懒连接+指数退避稳定性窗口) / tests.rs

database/          SQLite 全栈基础设施:mod.rs / schema.rs(SCHEMA_VERSION + apply_migrations()) / pragmas.rs(WAL + 6 PRAGMA) / paths.rs / models.rs / chat_store.rs(chat_sessions/chat_turns 整快照重写) / provider.rs(providers 五元组 + context_max_size) / mcp_server.rs(mcp_servers 接入记录 + headers Vault 加密)

config/            全局配置:mod.rs / agent_memory.rs(agent_memory DAO) / agent_message.rs(agent_message DAO) / session_memory.rs(session_memory DAO + SessionContext 摘要) / subagent_run.rs(动态子 Agent 运行记录 DAO:insert/finish/list/get/孤儿标记/trim)
```

统一消息模型是关键设计：Agent 循环与工具层永远不接触协议细节，协议差异封闭在 `llm/*` 两个客户端内部。

## TUI 界面（独立 CLI 渲染引擎）

### 屏幕拓扑

- **自适应底部面板（第 134 轮）**：输入面板不再恒定吸屏幕底部，而是**顶行紧跟当前光标行**（`Layout::panel_top_for` 纯函数决策，`enter_pinned` 落地）。启动横幅这类「短输出 + 大终端」场景实测原会固定留 16 行空白（观感像一串空回车），现输出与面板之间**恒不留空行**；内容接近屏底时自动吸底，行为与旧版一致。滚动区底行改为 `panel_top`（`scroll_bottom()`），输出填满时游标正好停在面板正上方。**坑**：查光标行必须**在 `ESC[r`(DECRST)之前**发 `ESC[6n` —— 本机 tmux 实测 `ESC[r` 会让紧随其后的 DSR 回包变成 `1;1`（shell 复现：无 `ESC[r` 回 `21;1`，加 `ESC[r` 无回包），据此定位会把面板顶到第 2 行并抹掉启动横幅。`crossterm::cursor::position()` 本机恒返回 `Ok((0,0))`，不可用，改用自实现 `query_cursor_row()`（`libc::poll` 限时 + 先排空 stdin 陈旧字节，失败返回 `None` → 保持吸底，不猜）。面板顶行经 `LAST_PANEL_TOP` 静态量传递，供 `teardown_pinned()` 精确清除。
- **人工介入 `/hitl` 应急通道（第 134 轮）**：弹窗接管期间 TUI 此前**完全不读 stdin**，弹窗一旦屏外/被遮挡/进程卡住就是 120s 死等。现并联一条**只认 `/hitl` 前缀**的行读（`tui/hitl_view.rs::read_hitl_escape_answer` + `HumanAssistHub::respond_via_tui_escape` + 事件 `AssistEvent::TuiEscapeAnswered`）：`/hitl 2t6x` 文本应答、`/hitl 3` 选项 3、`/hitl cancel` 取消，映射规则与既有行读完全一致；**其它任意输入行丢弃并提示**，绝不与弹窗抢应答。
- **人工介入弹窗「点击无响应」根治（第 134 轮）**：三条经探针+截图复现的独立缺陷已修 —— ① `macos_dialog.js` 的 `doLayout` 起始 `y = ch - titleH` 比 `contentHeight` 多扣一次标题，底部「取消/提交」整行下移 22px（实测 frame `y=-6`，内容视图高 300）被裁，改为 `y = ch`；② 最大化分支用 `setFrameDisplay`（frame 坐标，含 32px 标题栏）却按内容高排版，改用 `setContentSize` + `setFrameOrigin`；③ 弃用 `[NSWindow center]`（依赖 `window.screen` 隐式归属，后台 osascript 在多屏下实测落到主屏下方的副屏 `win.frame=(1352,-530,…)`，主屏内完全不可见），改为 `pickScreen()`（visibleFrame 面积最大）+ `placeWindow()` 显式落位。沿革详见 `docs/MCP_Web_Use/03-人工介入弹窗原生UI方案.md` §11。
- **REPL 主屏**：保留 `InputHandler` 单行输入 + 斜杠命令补全 + 多轮对话。**粘贴保真**：bracketed paste 整体接收；**≥2 行粘贴一律转 `[粘贴 #N +M 行]` marker 保真**（原文含换行入登记表，不再把换行压成空格——旧「>10 行才转」规则会让 3~8 行 Markdown 提示词在屏幕上只剩一行、送进模型也丢掉列表结构），注册后立刻在滚动区回显原文预览（前 4 行 + 「其余 N 行未显示」），提交时按**展开版**逐行完整回显（首行 `>> `、续行 `.. `、超 20 视觉行折叠并标注「内容已完整发送」）；单份 >10000 字符截断为首尾各 500 + 省略标注+ 快速输入批量合并（IME/旧终端粘贴逐字重绘优化）+ Enter 提交前 15ms 粘贴突发探测（无 bracketed paste 的 Windows 终端多行提示词被逐行拆成多次提交 — 见 `tui/input.rs::drain_paste_burst`）。
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
| `/sessions` (`hist`) | 列出**跨进程**可恢复的历史会话：每轮任务收口自动整快照落盘根目录 SQLite `chat_sessions`/`chat_turns`，自动保留最近 50 个 |
| `/resume [N\|id前缀]` | 恢复历史会话：重建 context（prompt 展开版 + assistant 上下文回填版）/transcript/累计用量三处一致，保持原 Session ID（`session_memory` 摘要链连续）；恢复前当前对话自动快照存分支；`latest` = 最近一次。CLI 侧 `laew --resume [N\|id]`（短参 `-c`） |
| `/offline` (`status`)| 查看连接状态(Online/Degraded/Offline 三态)与离线队列深度；离线模式 |
| `/cost` (`usage`)    | 查看会话用量与成本估算：累计四类 token / 缓存命中率 / 按内置参考价(2026-09)的成本分解与实记累计；模型无内置价时仅统计 token |
| `/workspace` (`ws`) | 查看工作区快照：git 分支/未提交变更/工程类型与工具链建议/顶层结构/6h 内最近改动；`/workspace refresh` 强制失效 TTL 缓存重采集 |
| `/export [path]`  | 导出当前会话为 Markdown（`.json` 后缀导出 JSON）；默认落工作目录 `laew-export-{时间戳}.md`，同名冲突自动 `-1` 后缀，显式路径已存在拒绝覆盖 |
| `/tasks` (`todo`, `todos`) | 列出当前 session 的 TODO 任务清单：表格形式(id/status/priority/content) |
| `/agents` (`subagents`) | 自感知动态子 Agent 面板：开关与上限(深度/并发/会话预算/迭代/超时)+ 内置 6 类名册与默认工具面 + **自定义类型段**（`.laew/agents/*.md`：id/标签/描述/工具面/来源文件 + 被忽略项与原因）+ 运行记录持久化状态 + 本会话已启动数/运行中 + 最近 10 个动态子 Agent 作业表(run_id/类型/状态/origin/工具数/耗时/tokens)。子命令 `/agents history [N\|all]`：SQLite 运行记录工作板视图(跨任务/跨会话,含孤儿作业计数与「如何续跑」提示) |
| `/audit` (`audits`) | 决策审计可视化：当前 session 的 5 决策点事件表格/统计/校验/清理。子命令：`/audit` 表格(最近 10 条)，`/audit last [N]` 详情(默认 5,上限 50)，`/audit stats` 按 (decision, agent) 分组聚合，`/audit verify` JSONL 完整性校验，`/audit clean [--keep N]` 清理旧 session 审计文件(默认保留 10)。TUI bootstrap 自动 trim，`/cost` 末尾追加审计摘要 |
| `/commands`       | 列出已加载的自定义斜杠命令与来源 |
| `/provider`       | 管理接入记录（默认进入 list 屏） |

### 自定义斜杠命令（D2）

Markdown Prompt 模板，两级发现：**项目级** `{工作目录}/.laew/commands/*.md` + **用户级** `~/.laew/commands/*.md`（同名用户级优先；内置命令不可遮蔽）。frontmatter 支持 `description` / `argument-hint`（缺失时描述取正文首行截 60 字符）；模板占位符 `$ARGUMENTS`（全量参数）与 `$1`-`$9`（位置参数，`$10` 原样保留）；无占位符但带参调用时末尾追加 `ARGUMENTS:` 块。补全列表内置在前、自定义在后，每行输入前自动重扫（命令文件增删即时生效）。实现见 `src/tui/commands.rs`。

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

### 选中效果视觉规范

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
- 新增**盒线/信息块**（横幅、`/help`、状态面板等）：一律走 `tui::textfit::InfoBox`，禁止硬编码边框长度或逐行手调填充空格（旧实现正是这样导致行宽参差）；宽度度量只允许 `textfit::width`/`char_width`（`input::display_width` 已转发至此），截断只用 `textfit::clip`（省略号计入预算）与 `clip_mid`（路径/URL 保尾）。
- 不引入新 crate（crossterm 已满足）。
- 子屏不直接写 stdout；通过 `Frame` → `engine::present` 统一绘制。

## 文档地图（docs/）

**核心设计文档**（按模块）：
- `docs/多Agent架构重构/` — 从 0 到 1 的分阶段解决方案（架构 / 任务分解 / 技术设计）
- `docs/TUI界面与CLI渲染引擎/` — 独立 CLI 渲染引擎 + Tab 表单 + `/provider` 系列交互（01-产品设计 / 02-技术设计 / 03-Tab表单与Provider操作设计）
- `docs/TUI交互优化与-f命令设计.md` — TUI 自动补全与 `-f` 文件参数的产品和技术设计
- `docs/TUI输入处理常见陷阱与修复记录.md` — 大粘贴防护 / 焦点守卫 / 多行提交等
- `docs/TUI自动化测试/` — TUI 子屏自动化测试方案：**tmux control-mode** 真 PTY 渲染
- `docs/TUIMarkdown富文本渲染/` — TUI Markdown 渲染设计

**Agent 设计**：
- `docs/YoloAgent设计/` — 双 Agent 架构 / Yolo 入口层 / 任务三档分类 / 任务拆解 设计（01 / 02 / 03 信息收集型工具面 + ReAct 延迟强制）
- `docs/PlanAgent` / `docs/Main-Work工具扩展与ReAct改造/` — Main-Work 流程层 ReAct 化与工具扩展
- `docs/SubAgentWork执行层ReAct与连续工作模式/` — 执行层 ReAct 强化 + 工具连续工作模式 + 无进展止损
- `docs/Quality-Check Agent` / `docs/SessionContext Agent` / `docs/Debug模式与DebugAgent设计/` — 各角色设计

**协议与请求层**：
- `docs/Anthropic协议系统提示词三段式/` — 8 角色 system 三段式重构（billing + identity + rules）
- `docs/协议抓包/` — 各 Agent 真实 HTTP 抓包（RequestBody/ResponseBody）
- `docs/其他Agent工具定义/` — claude-code / codex / hermes / openclaw / open-code / pi / WorkBuddy 等的工具定义，新增工具时先读这里

**三个 MCP 风格工具**（按工具）：
- `docs/MCP_Window_Use/` — 桌面窗口操控（01 主设计 + 02 鼠标键盘优先级链 + 03 连续工作模式 + 04 中键修饰键 + 05 macos_legacy 拆分 + 06 横向对比 + MacOS/Window 平台技术文档）
- `docs/MCP_Web_Use/` — 浏览器操控（01 主设计 + 02 人工介入与窗口可视化 + 03 人工介入弹窗原生UI + **04 网页取证纪律** + 05 可视化模式蒙层与人工操作拦截 + **06 复用已登录浏览器会话** + **07 页面管控三档模式与人工交互设计**）
- `docs/MCP_Use/` — 通用 MCP 服务调用
- `docs/浏览器CDP工具/` — CDP 技术参考（chromiumoxide 选型 / launch vs connect / BrowserManager 单例）

**上下文与会话**：
- `docs/Context设置与自动压缩设计/` — ContextMaxSize 上限 + Compact Agent 三档自动压缩 + 溢出三级恢复
- `docs/Yolo项目上下文注入/` — 项目说明文件五级链发现 + 每会话首次注入
- `docs/工作区感知与运行时环境注入/` — 工作区快照 + 8 角色 system brief + TUI 变更对比
- `docs/WorkFlowAgent设计与实现/` — Goal 状态机 + Squad 调度工作流层

**自感知 SubAgent 三件套**：
- `docs/自感知SubAgent动态启动/` — D114 基础委派
- `docs/自感知SubAgent自定义类型与运行持久化/` — D115 自定义类型 + 运行持久化
- `docs/自感知SubAgent工作流编排/` — D116 工作流编排

**Skill / HITL / 离线 / 审计**：
- `docs/Skill系统与渐进式披露/` — Skill 系统 P0 落地
- `docs/多轮对话问题知识库/` — 多轮测试问题库
- `docs/TODO任务清单/` — D19 TODO 任务清单
- `docs/自签名证书TLS适配/` — IP + 自签名证书 HTTPS 网关适配

**调研归档**：
- `docs/Agent源码调研/` — 15 个外部项目源码调研 + 跨项目缺口分析（按调研批次编号归档）
- `docs/Agent架构对比与参考.md` — 7 个项目(6 外部 + laew)的横向对比报告
- `docs/工程初始化方案/` — 从 0 到 1 的分阶段解决方案

**数据库与基础设施**：
- `docs/数据库重构与Provider导入导出/` — SQLite 全栈基础设施
- `docs/完整调用链分析/` — 端到端调用链

**测试与构建**：
- `docs/自动化测试-提示词文件列表/` — 10 维度 × 100+ 组多轮对话测试脚本
- `docs/自动化测试与Debug一体化/` — 测试与 Debug 集成
- `docs/Windows脚本支持/` — Windows 脚本支持（rebuild_restart_app.bat）

## 自动化测试

测试分三层，放在 `testReport/` 下：

| 层             | 入口                            | 用途                                                                             |
| -------------- | ------------------------------- | -------------------------------------------------------------------------------- |
| 单元测试       | `cargo test`                    | Rust 函数级覆盖（模块、解析、转换、工具）                                          |
| 端到端(CLI)    | `bash testReport/run_e2e.sh`    | mock LLM，跑 `-p` / `provider add                                                 | list | use | delete` / 协议 wire 校验 / **项目上下文注入(说明文件五级链,5b 节)** / TUI 管道冒烟 / **TUI 子屏 tmux 自动化** |
| TUI 子屏自动化 | `testReport/run_e2e.sh` 第 8 节 | tmux control-mode 真 PTY 渲染，验证 alternate screen + raw mode + Screen::title() |

### TUI 自动化：**优先使用 tmux control-mode**

`src/tui/mod.rs::run` 对 `atty()` 做了分流：

- **TTY**（包括 tmux 内）→ `InputHandler` 全交互（原始模式 + alternate screen + 子屏栈）。
- **非 TTY**（管道 / 重定向）→ 行读取回退，**子屏走 print 输出，不是真实渲染**。

> 因此：`/provider list`、`/provider add`、`/provider del` 等**子屏行为必须用 tmux**。
> 管道冒烟仅适合主屏纯文本命令（`/help` `/model` `/new` `/exit`）。

核心命令速查（完整封装见 `run_e2e.sh` 第 8 节，设计见 `docs/TUI自动化测试/`）：

```bash
# 1) 起后台会话并启动 TUI，固定 100x30
tmux new-session -d -s laew_e2e -x 100 -y 30 "$LAEW"
# 2) 发送按键（整串字面量必须 -l）
tmux send-keys -t laew_e2e -l "/provider list"
tmux send-keys -t laew_e2e Enter        # 回车
tmux send-keys -t laew_e2e Escape       # Esc 退子屏
# 3) 抓取面板到 stdout（不带 -e 剥离 ANSI，便于 grep）
SCREEN=$(tmux capture-pane -p -t laew_e2e)
# 4) 断言:echo "$SCREEN" | grep -F -q "/provider list"
# 5) 调试:tmux attach -t laew_e2e  可肉眼回放
# 6) 收尾:tmux kill-session -t laew_e2e
```

扩展指引（新增子屏断言）：

1. 在 `src/tui/screen/*` 找到 `fn title() -> &str`，title 字符串本身就是断言锚点。
2. 在 `run_e2e.sh` 第 8 节 `texpect "<title>" "..."` 即可。
3. 若断言失败，报告自动 dump 当前面板（带 `|` 前缀），便于排查。
4. CI 环境需 `apt-get install -y tmux`；缺 tmux 时整节 SKIP，不影响其它 9 节通过。

## 约定

- 注释、CLI 文案、文档一律中文；代码标识符英文。
- **单源码文件 ≤ 1800 行**：超过必须按功能/业务/架构维度拆分模块；临界文件（≥ 1700 行）新增代码优先落到职责子模块，防止越线。**拆分规范**：
  - 单文件超线 → 转同名目录（`xxx.rs` → `xxx/mod.rs` + 职责子模块）；`mod.rs` 承载结构体定义 + 构造器/入口 + `pub use` 再导出，**外部 `crate::…::xxx::Yyy` 路径零改动**；
  - 原私有项搬入子模块标 `pub(super)`（可见域 = 本目录子树，与拆分前单文件作用域等价），由 `mod.rs` 私有 `use` 重导入供兄弟模块 `use super::*` 取用；
  - 代码**逐行机械搬移零改写**（不重构逻辑/注释），仅新增模块文档头与 `use super::*`；测试搬 `tests.rs`（mod.rs 声明 `#[cfg(test)] mod tests;`），拆分前后测试数必须对账一致；
  - ⚠️ **`tests.rs` 不得转成 `tests/` 目录**：`.gitignore` 的 `tests/` 规则匹配**任意层级**的 tests 目录，`src/agent/tests/` 会被静默排除出版本库（本地编译通过、克隆后 `cargo test` 直接失败）。测试模块一律用 `xxx/tests.rs` **文件**；某个 `tests.rs` 自身超线时，把新用例放到**同级兄弟文件**（如 `src/agent/react_tests.rs` + `#[cfg(test)] mod react_tests;`），不要转目录；
  - 高速增长文件（近几轮每轮 +50 行以上）在破线前预防性拆分。已按此规范拆分：`src/tui/`（分层）、`src/agent/`（agent_loop + runtime_hints + tests 平铺拆分）、`agent/orchestrator/`、`agent/main_work/`、`agent/tools/window/`。
- 新工具：在 `src/agent/tools/` 建同名模块实现 `Tool` trait，注册进 `builtin_registry()`（Work Agent）或相应 registry，Schema 参考 `docs/其他Agent工具定义/`。
- 新协议：实现 `LlmClient` trait + `client_from_record()` 增加分支，不改动 agent 层。
- 新 Agent 类型：实现 `AgentProfile`（独立名称/系统提示词/工具集），在 `YoloRunner` 或相应编排器中接入。
- 测试报告输出到 `testReport/`（命名 `e2e-<时间戳>.txt` / `验证报告-<日期>.md`）；临时计划放 `tmpPlan/`（已 gitignore）。
- `laew`、`*.db`、`tmpPlan/`、`target/` 均不入库（见 .gitignore）。

## Git 提交规范

**格式**：`<type>(<scope>): <中文业务描述>`，沿用仓库既有风格（可选轮次后缀，如 `feat(web): 可视化模式页面蒙层与人工操作拦截(第141轮)`）。
`type` 取 `feat` / `fix` / `perf` / `refactor` / `test` / `docs` / `chore`；`scope` 用模块名（`web` / `tui` / `llm` / `qc` 等）。描述只写**做了什么业务改动**，不写过程与情绪。

**硬性禁止（生成 commit message 时一律不得出现）**：

1. 禁止添加任何 `Co-Authored-By: Claude Code <noreply@anthropic.com>` 行（以及其它形式的 AI 联合作者署名）。
2. 禁止追加 `Generated with Claude Code` 这类 AI 署名/生成声明文本（以及 `🤖 Generated with …` 等变体）。
3. commit message **只保留业务描述**，末尾不追加任何 AI 相关的 attribution 尾部注释（不写「本提交由 AI 生成」「由 Claude Code 协助」等）。

**其它约定**：

- 默认**不主动提交**：改完代码先向用户汇报 diff 摘要，等明确指示再 `git commit`。
- 需要提交时先确认在 `main` 之外的分支（默认分支上先建分支）。
- 提交前确保 `cargo build` 通过；改动范围大时同步跑 `cargo test`。