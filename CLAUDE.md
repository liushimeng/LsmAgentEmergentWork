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
| `LAEW_LOG_CLIP` | 正整数（默认 `4000`） | `--debug`/`--info` 运行日志文件中单字段（LLM 思考文本/工具参数/结果等）的截断长度（字符数）；`0`/非法值回退默认。见 `src/logging.rs` |
| `LAEW_AUDIT` | `off`/`0`/`false`/`no` | 关闭决策审计写入（默认开启）。开启时 5 个决策点（Yolo 分类 / Plan 规划 / Main-Work 拆解 / QC 判定 / Compact 压缩）各追加一条结构化 JSON 行到根目录 `AuditTrail/audit_{session_id}.jsonl`（已 gitignore），记录「输入上下文→决策结论→决策依据」三段式 + 耗时/token/扩展字段，全字段脱敏截断，fail-open 不影响主流程。见 `src/agent/decision_audit.rs` |
| `LAEW_PARALLEL_TOOLS` | `off`/`0`/`false`/`no` | 关闭「同批只读工具并发执行」，回退到全串行。默认开启：同一 LLM 响应里**连续的** `parallel_safe` 调用（Read/Glob/Grep，段长 ≥2）并发执行，`tool_result` 仍按原序回填。见 `src/agent/tool_exec.rs` |
| `LAEW_MAX_PARALLEL_TOOLS` | 正整数（默认 `4`） | 单批并发上限；`0`/非法值回退默认。对齐 AtomCode `ATOMCODE_MAX_PARALLEL_TOOLS=4`（Claude Code 用 10，laew 单元迭代预算小取保守值）。见 `src/agent/tools/mod.rs::max_parallel_tools_from` |
| `LAEW_REACT_GUARD` | `off`/`0`/`false`/`no` | 关闭 ReAct 循环守卫（无进展软提醒与 doom_loop 止损均不生效；`doom_loop_repeats` 仍统计，可观测性不丢）。默认开启：进展键 =「工具名 + 参数稳定 JSON + 结果摘要」三者全同即无进展，第 2 次软提醒 / 第 3 次宽限轮强提醒 / 第 4 次止损终止。见 `src/agent/loop_guard.rs` |
| `LAEW_MCP_ENABLED` | `off`/`0`/`false`/`no` | 关闭通用 MCP 服务调用（`MCP_Use` 工具注册与提示词同时归零，严格向后兼容）。默认开启：Agent 可经 `MCP_Use` 调用 `laew mcp add` 配置的外部 MCP server 工具/资源。见 `src/agent/tools/mcp_use/mod.rs` |
| `LAEW_SELF_SPAWN` | `off`/`0`/`false`/`no` | 关闭**自感知动态子 Agent**（默认开启）。关闭时不注册 `SubAgent` 工具、不注入自感知提示词段、不建运行时（工具面/提示词/耗时与改造前完全一致）。见 `src/agent/self_awareness.rs` |
| `LAEW_SUBAGENT_MAX_DEPTH` | 0..=3 | 动态子 Agent 嵌套层数上限，默认 `1`（子 Agent 为叶子，不能再启动）。`0` = 完全禁止（等价关闭）。 |
| `LAEW_SUBAGENT_MAX_PARALLEL` | 1..=8 | 会话级并发槽位，默认 `3`（对齐 atomcode `Semaphore(3)`）。 |
| `LAEW_SUBAGENT_MAX_TOTAL` | 1..=64 | 会话级累计启动预算，默认 `8`（防 token 失控；耗尽返回信封 `2001`）。 |
| `LAEW_SUBAGENT_MAX_ITERATIONS` | 4..=32 | 单个子 Agent 迭代上限，默认 `12`。 |
| `LAEW_SUBAGENT_TIMEOUT_SECS` | 10..=3600 | 单个子 Agent 墙钟超时，默认 `300`（超时记 `status=timeout`）。 |
| `LAEW_SUBAGENT_PERSIST` | `off`/`0`/`false`/`no` | 关闭动态子 Agent **运行记录持久化**（默认开启）。关闭后不写不读 SQLite `subagent_run` 表，`SubAgent(action="history")` 返回 `code=0 + persist:false + 空列表`（工具面与提示词不变；`LAEW_SELF_SPAWN=off` 时本就零写入）。见 `src/config/subagent_run.rs` |
| `LAEW_SUBAGENT_RUN_KEEP` | 0..=20000 | 启动期保留的最新运行记录条数，默认 `500`（`0` = 不清理）。TUI bootstrap 与 `-p`/`-f` 单轮模式启动时各执行一次 trim（fail-open）。 |

## 领域概念（改代码前必读）

- **根目录** = `laew` 二进制所在目录（`current_exe()` 父目录）。数据库 `LsmAgentEmergentWork.db`、编译产物 `./laew` 都在这里。
- **工作目录** = 启动命令时所在目录。Bash/Read/Write 工具的相对路径基准。两者可能不同，勿混淆。
- **当前项目说明文件** = 以**工作目录**为基准按五级链发现：非空 `CLAUDE.md` → 非空 `AGENTS.md` → 非空 `README.md` →（都没有但根目录层有其它 `*.md` 时，程序化分析后**自动生成 `README.md`** 落盘使用）→ 空（不注入）。Yolo 在每个 Session **首次处理**时，把「工作目录路径 + 说明文件内容」包装成带 `<<<LAEW:PROJECT_CONTEXT>>>` 标记的独立 user 消息插入上下文 index 0（标记探测幂等、与用户提示词严格隔离），设计见 `docs/Yolo项目上下文注入/`。TUI 横幅的「项目说明:」行为纯探测展示。
- **接入记录（完整的大模型接入记录）** = `protocol(anthropic|openai) + provider_name + model_name + end_point + api_key` 五元组 + `context_max_size`(上下文最大 Token 数,默认 800K,`0` = 不限制/关闭自动压缩；支持 `800000`/`800K`/`1M` 写法)，存 SQLite `providers` 表，可多条，`is_active` 唯一。存量库打开时自动迁移补列回填默认值。
- **接入点补全**：Anthropic → `{end_point}/v1/messages`；OpenAI → `{end_point}/chat/completions`；尾部 `/` 自动裁剪。
- **运行日志文件（输出 log 文件）**：`--debug` / `--info`（含 `-debug` / `-info` 单横线与 `--DEBUG` 等大小写变体）在工作目录生成 `llaew_YYYYMMDD_HHMMSS.log`（时间戳 = laew 启动时刻，精确到秒，本地时区；已 gitignore）。`--debug` → DEBUG 级（含每轮 LLM 请求元信息/响应思考文本与工具意图全文），`--info` → INFO 级主干事件；实现复用 tracing 双层订阅器（原控制台层行为不变 + 文件层 `src/logging.rs`），埋点覆盖全部 8 角色的感知（任务输入/Agent 会话）/ 决策（Yolo 分类/Plan/Main-Work 拆解/QC 报告）/ 执行（WorkFlow 单元/Context 压缩/SessionContext 摘要/任务收口）/ 思考（LLM 响应文本与 tool_calls）与全部工具调用（名称/参数/结果/耗时，`agent_loop.rs` 中央埋点）。TUI 横幅追加「启动时间」行（常显）与「日志文件」行（仅 `--debug`/`--info` 时显示相对化路径 + 级别，`/clear` `/new` 重印横幅仍可见；启动时刻由 main 单点捕获，横幅显示与日志文件名时间戳严格同刻，`TuiLaunch` 传递）。
- **工具定义协议差异**：Anthropic 用 `tools[].{name,description,input_schema}`；OpenAI 用 `tools[].{type:"function",function:{name,description,parameters}}`（function 风格）。

### 多 Agent 架构（8 角色）

| 角色 | 身份 | 职责 | 工具面 |
|------|------|------|--------|
| **Yolo Agent** | `LsmAgentEmergentWork-Yolo` | 入口层：每条输入做 目的→目标→意图 三步分析；任务**三档分类**（simple/medium/hard）；失败回流与用户建议；分类前按 ReAct 自主收集信息（避免 Google 类目的）；延迟强制 `submit_task_classification`（探索轮不注入 forced `tool_choice`、仅末轮强制收口） | `Read`/`Glob`/`Grep`/`Bash`（只读侦察）/ `MCP_Web_Use`（观察类 action）/ `SubAgent`（只读并行子 Agent） |
| **Plan Agent** | `LsmAgentEmergentWork-Plan` | 规划层：仅在 hard 任务时启用；输出 Markdown 方案到 `plans/{session_id}-{seq}.md` | `Read`/`Write` |
| **Main-Work Agent** | `LsmAgentEmergentWork-Main-Work` | 流程层：接收 medium/hard 任务，拆 WorkFlow 列表（Kahn 分层 + 同层并行）；编排循环 ReAct 化（TaskFocus → Verify+Decompose → Emit 三段式 + 任务前提验证硬性要求）；复用 LoopGuard 编排层原地打转同样止损；迭代预算 `max_iterations(8)` + `explore_budget(2)` | `Bash`/`Read`/`Glob`/`Grep`/`MCP_Web_Use`（编排前探查）/ `TodoWrite`/`SubAgent`（**不持** `Write`/`Edit`，流程层只编排不落源代码；不持 `MCP_Window_Use`，桌面窗口操控归 SubAgent-Work 专用） |
| **SubAgent-Work Agent** | `LsmAgentEmergentWork-SubAgent-Work` | 执行层最小单元，每个流程处理单元委派一个 SubAgent；提示词 ReAct 化（Thought→Action→Observation）+ 工具连续工作模式（同一响应里连续的 `parallel_safe` 工具 Read/Glob/Grep 用 `join_all` + `Semaphore(4)` 并发执行 + `tool_result` 保序回填）+ 无进展止损（`agent/loop_guard.rs` 进展键 = 工具名 + 参数稳定 JSON + 结果摘要，轮询等待类合法重复零误伤；`wait_like()` 让 wait/纯 sleep/`SubAgent(result|history)` 透明跳过；`NUDGE_AT=2`/`ABORT_AT=3` 双阈值 + 宽限轮强提醒作为 user 消息延迟到 tool_result 回填完再推入避免破坏 Anthropic 400 配对）；runtime hints 角色化（`HintRole{Ui,Execute,Gather,Judge}` 由实际调用过什么工具决定） | `Bash`/`Read`/`Write`/`Edit`/`Glob`/`Grep` + 平台门控注入 `MCP_Window_Use`（仅 macOS/Windows）+ `MCP_Web_Use` + `MCP_Use`（通用 MCP）+ `TodoWrite` |
| **Quality-Check Agent** | `LsmAgentEmergentWork-Quality-Check` | 质检层：每个执行单元完成后必经 QC；QC LLM 错误与用户取消不消耗单元 retry 预算 | 可选 `Read` |
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
| **MCP_Web_Use** | 原 Chromium-WebUse Agent | 浏览器操控：单工具 `action` 枚举分发（`open`/`list`/`close`/`control`/`inspect`/`sequence`/`batch`/`explore`）；`control_action` 39 个写操作（鼠标/键盘/拖拽/上传/下载/eval_js/Cookie/视口/截图等）；`inspect` 16 个观察维度（含 Console/Network/Elements/DOM/localStorage/Cookie/页面元信息/OCR/`blockers` 人工阻断检测）；单步 + 连续（`sequence` ≤24 steps，批内 `$page_id`/`$spawned_page_id` 占位自动跟随派生新页）；视口基准 1080p + 2K 自动扩展（Playwright/Puppeteer 同款机制，CDP 坐标恒 CSS 像素无 DPR 换算）；验证码 OCR（macOS Vision）+ eval_js 容错与结果净化（自动 IIFE 重试 + 大结果/data-url 落盘）+ CDP 下载管理（`data:` 直存/`about:/blob:/javascript:` 快速失败）；headed 模式默认 1080p 窗口 + 蓝色选中边框与「LAEW Agent 控制中」徽标；**人工介入 HITL**（`src/agent/human_assist.rs` 全局枢纽，滑块/短信/扫码登录等不可自动跳过流程向 TUI 发起结构化选择，人工答复经 oneshot 回填；非 TUI 模式 fail-fast 4001/4002） | 跨 Windows/macOS/Linux；Chrome→Edge→Chromium→Brave 自动探测 | `docs/MCP_Web_Use/01-设计与解决方案.md` |
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
  3. **rules 核心行为规则**（带 `cache_control: ephemeral`）：base + tools_hint + protocol_tail + 运行时 workspace_hint + runtime_hints，等价于原 `render()` 单字符串内容
  缓存复用：billing + identity 静态常量跨会话完全一致 → 100% 命中；rules 因 runtime hints 注入每轮不同 → 每轮重算但仍带 cache 标记。4 断点 cap 约束下，billing 无 cache / identity + rules + last tool + latest user 各一份 cache，刚好命中 cap。OpenAI 协议不受影响，继续走 `system` 单字符串路径。实现 `src/agent/system_prompt/mod.rs::PromptSegments` + `src/llm/anthropic.rs::convert_system_blocks_split`，Agent 循环 `meta.anthropic_segments = Some(profile.system_prompt.prompt_segments())` 注入。

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
  format.rs        纯函数格式化:任务结果双版本(人类版 format_task_result / LLM 回填版 for_context)/ merge_usage / waiting_line_text / CJK 截断系 / print_help / print_record
  format_brief.rs  [tool]/[laew] 行关键字段摘要(MCP_Window_Use 焦点守卫/route/verified 等)
  audit_view.rs    /audit 决策审计可视化面板
  engine.rs        CLI 渲染引擎 —— Screen trait + Frame + 全量重绘 present
  form.rs          通用 Tab 表单状态机(被 ProviderForm 屏复用)
  input.rs         单行输入(主屏用):含行内提示 + 补全 + D6 大粘贴防护(crossterm 原始模式)
  completion.rs    斜杠命令补全引擎(内置 + 自定义命令动态注册)
  commands.rs      自定义斜杠命令(D2):两级目录发现/frontmatter/占位符渲染
  export.rs        会话导出(D8):transcript 记录 + Markdown/JSON 落盘
  branches.rs      对话分支存储(D3):rewind/fork/switch/clear 前自动快照(内存态上限 10)
  mention.rs       @ 文件提及解析 + 实时路径补全 + 目录钻取
  pathfmt.rs       路径格式化辅助(相对路径展示/工作目录锚定)
  theme.rs         ANSI 颜色 / mask_key 脱敏 / attrs·bg·color→ANSI 转换 集中管理
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
  human_assist.rs  人工介入 HITL(D100):oneshot 通道 + TUI 选择 + 非 TUI 模式 fail-fast
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
  safety/          安全防护:mod.rs / url_safety.rs(SSRF 拦截) / prompt_injection.rs(提示注入检测) / credentials.rs(凭证脱敏)
  sandbox_hook/    沙箱钩子:mod.rs(单文件,接外部 sandbox)
  skills/          Skill 系统(渐进式披露):mod.rs / registry.rs / render.rs / tools.rs / skill.rs / bundled.rs / bundled/{code-review,git-commit,test-runner}.md

  system_prompt/   SystemPrompt 组合与渲染:mod.rs / mcp_use_hint.rs / skill_catalog.rs
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
    mcp_web_use/     MCP_Web_Use 工具目录:mod.rs(门面+八 action 分发+视口自动扩展接线) / control.rs(39 个写操作) / inspect.rs(16 个只读观察) / tests.rs
    mcp_use/         MCP_Use 工具目录(通用 MCP 服务调用):mod.rs(门面+七 action 分发+JSON 信封 0/1001/3001/5001-5006) / tests.rs

  window/          窗口操控平台驱动层(MCP_Window_Use 服务实现):mod.rs(模型+WindowDriver trait+工厂) / windows.rs(UIA+Win32) / windows_input.rs / windows_ocr.rs / macos_axui.rs(AX) / macos_vision_ocr.rs(Vision OCR) / control_action.rs / fallback.rs(wmctrl/xdotool) / macos_legacy/(mod.rs FFI+Driver入口 / ax_attrs.rs / cg_event.rs CGEvent 输入底座 / inspect.rs AX 控件树遍历 / act.rs / tests.rs)
  browser.rs       浏览器 CDP 驱动层(MCP_Web_Use 服务实现):BrowserManager 单例(page_id 注册表 + 跨平台浏览器检测 + Console/Network 缓冲 + 下载事件管理 + 生命周期回收) + 全模式 1080p 启动基线与 viewport_fit_plan/fit_viewport_to_content 2K 自动扩展
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

- **REPL 主屏**：保留 `InputHandler` 单行输入 + 斜杠命令补全 + 多轮对话。**大粘贴防护**：bracketed paste 整体接收 + 大粘贴（>10 行或 >1000 字符）转 `[粘贴 #N]` marker、提交时展开还原（单份 >10000 字符截断为首尾各 500 + 省略标注）+ 快速输入批量合并（IME/旧终端粘贴逐字重绘优化）+ Enter 提交前 15ms 粘贴突发探测（无 bracketed paste 的 Windows 终端多行提示词被逐行拆成多次提交 — 见 `tui/input.rs::drain_paste_burst`）。
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
- `docs/MCP_Web_Use/` — 浏览器操控（01 主设计 + 02 人工介入与窗口可视化）
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