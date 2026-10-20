# AGENTS.md — LsmAgentEmergentWork

供 AGENTS.md 约定类 AI Agent Tools（Codex / OpenCode / pi / OpenClaw 等）自动加载的工程入口说明。
**模块化、功能化导向**：本文档只描述"工程当前长什么样"与"开发约定"，不累加"在第 X 轮新增了 Y"类开发日志。
**与 [`CLAUDE.md`](./CLAUDE.md) 内容一致**（两份各自独立成文件、正文同步维护；改任一份请同步另一份）。

## 工程是什么

由 LLM 驱动的 Rust Agent CLI（二进制名 **`laew`**）。支持 **Anthropic**（anthropic-messages）与
**OpenAI**（openai-completions）双协议，**多 Agent 架构**（8 角色 + 三档难度），
内置 Bash / Read / Write / Edit / Glob / Grep 六个核心工具 + MCP_Web_Use 浏览器操控
+ MCP_Window_Use 桌面窗口操控 + Use_MCP 通用 MCP 服务调用（真 MCP 协议客户端） + SubAgent 自感知动态委派
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
cargo test                               # 单元 + 集成测试全量
cargo test --lib                         # 只跑单元测试
cargo test --test protocol_wire          # 只跑某个集成测试文件
cargo test --test cli_provider -- --nocapture   # 失败时打印子进程输出
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
| | 未设置（默认） | 自动模式：endpoint 主机为 IP 时自动跳过证书校验，域名主机仍严格校验。设计见 `docs/自签名证书TLS适配/01-设计与解决方案.md` |
| `LAEW_ALLOW_PRIVATE_ENDPOINT` | `1` | SSRF 防护放行私网/loopback endpoint（本地 Ollama / 局域网 / mock 测试 provider 用；默认拦截，见 `src/agent/safety/url_safety.rs`） |
| `LAEW_BASH_UTF8` | `1`/`true`/`yes`/`on` | Bash 工具为子进程注入 UTF-8 环境，消除 Windows GBK 乱码；默认关闭。见 `src/agent/tools/bash.rs` |
| `LAEW_ARTIFACT_DIR` | 任意路径 | 覆盖**产物根目录**（缺省 = 启动时工作目录 > 程序目录）。八类产物全落这里。见 `src/artifact_root.rs` |
| `LAEW_AMBIGUOUS_WIDE` | `1`/`true`/`yes`/`on` | 歧义宽度字符按 2 列参与宽度计算（默认关闭=按 1 列）。见 `src/tui/textfit.rs::ambiguous_wide` |
| `LAEW_LOG_CLIP` | 正整数（默认 `4000`） | 运行日志文件中单字段的截断长度（字符数）。见 `src/logging.rs` |
| `LAEW_AUDIT` | `off`/`0`/`false`/`no` | 关闭决策审计写入（默认开启）。见 `src/agent/decision_audit.rs` |
| `LAEW_PARALLEL_TOOLS` | `off`/`0`/`false`/`no` | 关闭「同批只读工具并发执行」，回退到全串行。见 `src/agent/tool_exec.rs` |
| `LAEW_MAX_PARALLEL_TOOLS` | 正整数（默认 `4`） | 单批并发上限。见 `src/agent/tools/mod.rs::max_parallel_tools_from` |
| `LAEW_REACT_GUARD` | `off`/`0`/`false`/`no` | 关闭 ReAct 循环守卫(工具级无进展检测)。见 `src/agent/loop_guard.rs` |
| `LAEW_DEAD_LOOP` | `off`/`0`/`false`/`no` | 关闭 LLM 交互级死循环守卫(裁决 + QC 根因裁决通道),**检测统计仍保留**。见 `src/agent/dead_loop/` |
| `LAEW_DEAD_LOOP_LONG_WINDOW` | 正整数(默认 `12`) | 大循环「低新颖度」检测窗口长度(轮) |
| `LAEW_MAINWORK_TIMEOUT` | 正整数(秒,默认 `300`) | Main-Work 拆解墙钟超时。见 `src/agent/orchestrator/pipeline.rs` |
| `LAEW_TOOL_TIMEOUT_SECS` | 正整数(秒,默认 `300`) | 单条工具调用总执行超时。豁免:`Bash`与 `request_human`。见 `src/agent/tool_exec.rs` |
| `LAEW_WF_SERIAL_FALLBACK` | `off`/`0`/`false`/`no` | 关闭零依赖计划串行链兜底。见 `src/agent/main_work/topo.rs` |
| `LAEW_REPLAN` | `off`/`0`/`false`/`no` | 关闭 Plan 再规划/更新规划。设计见 `docs/PlanAgent再规划与更新规划/01-设计与解决方案.md` |
| `LAEW_DEBUG_SYNC` | `1`/`true`/`yes`/`on` | 恢复 `--debug` TUI 同步生成调试报告（默认后台化）。见 `src/tui/dispatch.rs::spawn_debug_report` |
| `LAEW_REFLECTION` | `off`/`0`/`false`/`no` | 关闭 Reflection（反思）模式。见 `src/agent/reflection.rs`、`docs/Reflection反思模式/01-设计与解决方案.md` |
| `LAEW_REFLECTION_MODE` | `deep`（默认）/ `light` | 反思档位：`deep` = 条件触发 LLM 深度反思；`light` = 零 LLM 只跑机械教训 |
| `LAEW_REFLECTION_BUDGET` | 正整数（默认 `2`） | 每单元深度反思次数上限 |
| `LAEW_MCP_ENABLED` | `off`/`0`/`false`/`no` | 关闭通用 MCP 服务调用。见 `src/agent/tools/use_mcp/mod.rs` |
| `LAEW_SELF_SPAWN` | `off`/`0`/`false`/`no` | 关闭自感知动态子 Agent。见 `src/agent/self_awareness.rs` |
| `LAEW_TARGET_ANCHOR` | `off`/`0`/`false`/`no` | 关闭任务锚点全部四层。详见「领域概念 · 任务锚点」 |
| `LAEW_TARGET_ANCHOR_BLOCK` | `off`/`0`/`false`/`no` | 仅关闭 L3 工具级阻断，保留 L1/L2/L4（观察模式） |
| `LAEW_BROWSER_MODE` | `headed`/`hidden`/`new_headless` | 浏览器启动模式显式覆盖。缺省有 GUI→`headed`，无 GUI→`hidden`。见 `src/agent/browser_mode.rs` |
| `LAEW_WEB_HEADLESS_SOFTRENDER` | `on`/`1`/`true`/`yes` | 无头档恢复旧软渲染参数集（`--disable-gpu` + `--hide-scrollbars`）。见 `docs/MCP_Web_Use/37` |
| `LAEW_WEB_EVIDENCE` | `off`/`0`/`false`/`no` | 关闭网页取证纪律硬闸门。见 `src/agent/safety/web_evidence.rs`、`docs/MCP_Web_Use/04-网页取证纪律.md` |
| `LAEW_WEB_FINGERPRINT` | `off`/`0`/`false`/`no` | 关闭环境指纹体检与保真回填（legacy 杀开关；新代码用 `LAEW_WEB_STEALTH`）。见 `src/agent/fingerprint.rs` |
| `LAEW_WEB_STEALTH` | `hardened`（缺省）/`fidelity`/`off` | 反检测档位。**侵入式 `hardened`（缺省）** = 保真档全部 + `Function.prototype.toString` 原生化。**非侵入式 `fidelity`** = 只移除自动化泄漏 + 回填本机真值。`off` = 零处置。见 `src/agent/stealth.rs`、`docs/MCP_Web_Use/26` + `31` |
| `LAEW_WEB_SPAWN_ARM` | `off`/`0`/`false`/`no`/`关闭` | 关闭派生页早期武装 watcher。见 `src/agent/browser_spawn_arm.rs`、`docs/MCP_Web_Use/34` |
| `LAEW_READ_IMAGE_B64` | `1`/`true`/`yes`/`on` | 恢复 Read 工具图片 base64 全文返回（缺省为存根）。见 `src/agent/tools/read.rs` |
| `LAEW_WEB_OVERLAY` | `off`/`0`/`false`/`no` | 关闭可视化蒙层与人工操作拦截（legacy 开关）。见 `src/agent/browser_overlay.rs` |
| `LAEW_WEB_GUARD` | `locked`（缺省）/ `open` | 页面管控缺省档位。详见「领域概念 · 页面管控三档」 |
| `LAEW_WEB_LOCKED_SCROLL` | `off`/`0`/`false`/`no` | 关闭 locked 档「可滚动观看」DOM 输入闸。见 `src/agent/browser_overlay.rs` |
| `LAEW_WEB_HITL_SCOPE` | `off`/`0`/`false`/`no` | 关闭凭证区域探测。见 `src/agent/tools/mcp_web_use/unlock_zone.rs` |
| `LAEW_WEB_BLOCKER_PROBE` | `off`/`0`/`false`/`no` | 关闭自动阻断感知。详见「领域概念 · 自动阻断感知」。见 `src/agent/tools/mcp_web_use/blocker_probe.rs` |
| `LAEW_HITL_AUTO` | `off`/`0`/`false`/`no` | 关闭人工介入自动弹出（auto-HITL）。见 `src/agent/tools/mcp_web_use/control/hitl_ctl.rs` |
| `LAEW_WEB_EVAL_TIMEOUT_MS` | 正整数（默认 `30000`） | eval_js 执行超时兜底。见 `src/agent/tools/mcp_web_use/mod.rs` |
| `LAEW_WEB_MAX_WINDOWS` | 正整数（默认 `4`） | 多窗口并行上限。见 `docs/MCP_Web_Use/22-多窗口并行双站同时工作.md` |
| `LAEW_WEB_MAX_TABS` | 正整数（默认 `8`，下限 `2`） | 标签页总量上限 |
| `LAEW_WEB_GEO` | `grant`（默认）/ `deny` / `prompt` | Chrome 原生地理位置授权弹窗自动处置。见 `src/agent/browser_perm.rs` |
| `LAEW_WEB_BACKGROUND_GATE` | `off`/`0`/`false`/`no` | 关闭多标签页后台挂起治理。见 `src/agent/tools/mcp_web_use/page_activity.rs` |
| `LAEW_WEB_FOCUS` | `keep`（默认）/ `always` / `never` | 焦点租约。见 `src/agent/browser_focus.rs` |
| `LAEW_WEB_PACE` | `off`/`0`/`false`/`no` | 关闭站点导航节奏治理。见 `src/agent/browser_pace.rs` |
| `LAEW_WEB_WIPE_COOLDOWN_SECS` | 正整数（默认 `90`） | 整页抹除冷却时长 |
| `LAEW_WEB_WIPE_TOTAL_THRESHOLD` | 正整数（默认 `3`） | 整页抹除总账阈值 |
| `LAEW_SITE_RISK_PERSIST` | `off`/`0`/`false`/`no` | 关闭站点反爬声誉跨会话持久化。见 `src/config/site_risk.rs` |
| `LAEW_SITE_RISK_TTL_DAYS` | 正整数（默认 `7`） | 站点声誉记录新鲜度窗口（天） |
| `LAEW_SITE_RISK_PREFLIGHT` | `off`/`0`/`false`/`no` | 关闭站点可达性预检门。见 `src/agent/orchestrator/pipeline.rs` |
| `LAEW_WEB_RISK_COOLDOWN_SECS` | 正整数（默认 `300`） | 风控挑战冷却时长 |
| `LAEW_SUBAGENT_MAX_DEPTH` | 0..=3 | 动态子 Agent 嵌套层数上限，默认 `1` |
| `LAEW_SUBAGENT_MAX_PARALLEL` | 1..=8 | 会话级并发槽位，默认 `3` |
| `LAEW_SUBAGENT_MAX_TOTAL` | 1..=64 | 会话级累计启动预算，默认 `8` |
| `LAEW_SUBAGENT_MAX_ITERATIONS` | 4..=32 | 单个子 Agent 迭代上限，默认 `12` |
| `LAEW_SUBAGENT_TIMEOUT_SECS` | 10..=3600 | 单个子 Agent 墙钟超时，默认 `300` |
| `LAEW_SUBAGENT_PERSIST` | `off`/`0`/`false`/`no` | 关闭动态子 Agent 运行记录持久化。见 `src/config/subagent_run.rs` |
| `LAEW_SUBAGENT_RUN_KEEP` | 0..=20000 | 启动期保留的最新运行记录条数，默认 `500` |
| `LAEW_CHROME_DEBUG_PORT` | 正整数（默认 `9222`） | 复用已登录浏览器的 CDP 调试端口探测覆盖。见 `src/agent/browser_reuse.rs` |
| `LAEW_WIN_THEME` | `off`/`0`/`false`/`no` | 关闭 Windows 弹窗 macOS 风格主题。见 `src/agent/human_ui/windows_theme.rs` |

## 领域概念（改代码前必读）

- **程序目录（原「根目录」）** = `laew` 二进制所在目录（`current_exe()` 父目录）。**只**用于定位程序自带资源，**不再是任何产物的落点**。
- **工作目录** = 启动命令时所在目录。Bash/Read/Write 工具的相对路径基准。两者可能不同，勿混淆。
- **产物目录** = `LAEW_ARTIFACT_DIR` > 启动时工作目录 > 程序目录（`src/artifact_root.rs`）。**所有运行期产物一律落这里**。**例外：SQLite `LsmAgentEmergentWork.db` 仍在程序目录**。启动横幅有独立「产物目录:」行。
- **当前项目说明文件** = 以**工作目录**为基准按五级链发现：非空 `CLAUDE.md` → 非空 `AGENTS.md` → 非空 `README.md` →（都没有但根目录层有其它 `*.md` 时，程序化分析后**自动生成 `README.md`** 落盘使用）→ 空（不注入）。Yolo 在每个 Session **首次处理**时注入 `<<<LAEW:PROJECT_CONTEXT>>>` 标记消息，设计见 `docs/Yolo项目上下文注入/`。
- **接入记录** = `protocol(anthropic|openai) + provider_name + model_name + end_point + api_key` 五元组 + `context_max_size`（默认 800K，`0` = 不限制），存 SQLite `providers` 表，可多条，`is_active` 唯一。
- **运行日志文件**：`--debug`/`--info` 在工作目录生成 `llaew_YYYYMMDD_HHMMSS.log`（已 gitignore）。实现见 `src/logging.rs`。
- **工具定义协议差异**：Anthropic 用 `tools[].{name,description,input_schema}`；OpenAI 用 `tools[].{type:"function",function:{name,description,parameters}}`。
- **任务锚点（TargetAnchor，`safety/target_anchor.rs`）**：从用户原文机械抽取的目标硬约束（**不经 LLM**），全局单例。跨五层设防：L0 输入保真；L1 编排器澄清门；L2 Main-Work 提示词约束 + 澄清单元阻断；L3 `MCP_Web_Use` 显式 URL 越界返 `code=6001`；L4 QC 目标一致性硬门 + `target_drift` 强信号。开关 `LAEW_TARGET_ANCHOR=off` 全关；`LAEW_TARGET_ANCHOR_BLOCK=off` 仅关 L3 阻断。
- **网页取证纪律**：网页任务的证据只能来自 `MCP_Web_Use`——不可达时**如实报告并停止**，禁止改用 `curl/ping/nc` 等 Bash 网络命令探测任务目标站点。三层设防：(1) 提示词约束；(2) 运行时硬闸门 `src/agent/safety/web_evidence.rs`；(3) `LAEW_WEB_EVIDENCE=off` 全关回退。
- **人工介入可见性**：`request_human` 的「该走人工」响应附 `next_action` + 完整 `human_assist` 载荷；输入硬闸 6002 与收尾探针首次命中则**直接自动弹出**（auto-HITL）。「取消任务」是真取消（返 `code=4002` 并置位进程级旗标 `human_cancel`）。`request_human` 不强制 `page_id`（无页可解锁时走任务级提问）。
- **人工介入弹窗 = Rust 原生子进程（`human_ui/`）**：`laew __hitl-dialog <payload.json>` 隐藏子进程——macOS 走 AppKit FFI、Windows 走 Win32。铁律：布局是 Rust 纯函数单一真源，最大化/还原只用自身设定值重排、**永不回读窗口 frame**。支持选项多选、auto-HITL 自动弹出、焦点不抢夺、Windows macOS 风格主题。
- **TUI 多行粘贴保真**：bracketed paste 整体接收，≥2 行粘贴一律转 marker 保真。Windows conhost / 部分 ConPTY 不支持 bracketed paste，通过三级粘贴判定（40ms 队列探测 → 击键节奏武装 → 正常提交）根治多行提示词被逐行拆成多次提交。实现见 `tui/paste.rs` + `tui/input/mod.rs`。
- **死循环守卫(两级)**：`src/agent/loop_guard.rs` 是**工具级**守卫(判定键 = 相邻两轮的 工具+参数+结果,抓「同动作同结果」);`src/agent/dead_loop/` 是**LLM 交互级**守卫(判定键 = 「模型已见信息集合」指纹 + 输出全量指纹,跨轮次做尾部周期扫描 + 低新颖度检测),补齐前者的三条盲区:换序打转 `A→B→A→B`、换工具打转、上下文自锁。两级同轮命中时**工具级优先**(证据更硬、可逐项复算)。止损后的根因归类与换路径裁决由 Quality-Check Agent 承担(`quality/dead_loop.rs::check_dead_loop`)。设计见 `docs/LLM交互死循环指纹检测与质检裁决/01-设计与解决方案.md`。
- **终端守卫**：`src/tui/term_guard.rs`。终端窗口关闭后 crossterm `event::read` 永久 hang，通过独立看门狗线程轮询 `POLLHUP` + 0 超时挂断探测根治。
- **注入命名空间隐身**：向页面注入的 guard/highlight/follow/探测脚本全部改用进程级随机命名空间（`src/agent/browser_ns.rs`），脚本尾部把全部 API 入口改**不可枚举**。第 179 轮起窗口面 API 挂 `Symbol.for` 命名空间对象、DOM 面用独立 token。
- **反检测档位（`src/agent/stealth.rs`）**：三档 `off`/`fidelity`/`hardened`（缺省）。注入脚本由三块按档组合（`CORE_JS` → `FIDELITY_JS` → `HARDENED_JS`），`stealth_js_for(mode)` 唯一真源。三条铁律：① 被改写属性/函数的描述符形态必须与原生一致；② 同档重复注入必须零动作（幂等闸）；③ 回填值必须与原生对象形态同洽。**隐身脚本必须在 `page.goto` 之前注册**。connect 模式（接管用户浏览器）零注入契约不变。
- **页面管控三档（guard，`browser_overlay.rs`）**：`locked`（缺省，人工可看不可点）/ `open`（非屏蔽）/ `partial`（部分屏蔽，DOM 盾区）。locked 缺省用 DOM 选择性事件闸（只拦 `isTrusted=true`，放行滚轮/滚动），闸注入失败 fail-closed 回退 CDP。工具面 `open(guard?/allow_selectors?/block_selectors?)` + `control(set_guard)`。
- **人工介入弹窗 UI**：见 `docs/MCP_Web_Use/03` + `27`。Windows 弹窗 macOS 风格主题唯一真源 `src/agent/human_ui/windows_theme.rs`。
- **站点频控节奏治理（`browser_pace.rs`）**：两条防线：① 同注册域两次显式导航最小间隔（默认 2.5s）；② 频控横幅命中即标记同域冷却（默认 240s），冷却期内同域导航返 **6004**。杀开关 `LAEW_WEB_PACE=off`。
- **整页抹除(site_wipe)治理**：部分站点对 CDP 会话做会话/指纹级打标，被标会话每次页面完整加载后数秒内被站点脚本 `location→about:blank` 整页抹除。治理：连击熔断 + 总账跨实例持久化（`site_risk` 表）+ 延迟抹除复查 + 空白页清扫。`close(page_id="all")` 回收浏览器时冷却与连击清零（但总账不清）。
- **自动阻断感知与输入硬闸（`blocker_probe.rs`）**：风控弹层的题面常常没有"验证码"关键词，所以不能只做关键词匹配。① 推送式探针：输入类动作/`wait`/导航类/`eval_js` 收尾各探测一次，按结构打分；② `inspect(blockers)` 双通道 = 全文关键词 ∪ 弹层语义；③ 输入前置硬闸 6002：只认结构证据，命中即拦截；④ 子类型区分：只有真能填码的形态才叫 `captcha`；⑤ auto-HITL 自动弹出。杀开关 `LAEW_WEB_BLOCKER_PROBE=off`。
- **多标签页后台挂起治理（`page_activity.rs`）**：Chrome 一个窗口只有一个激活标签页，其余转后台挂起。① `control(screenshot)`/`inspect(screenshot|ocr)` 在后台页执行前返 **6003**；② 其余动作放行但响应附 `page_background` 提示；③ `list` 逐页 `foreground` 标记；④ `close(page_id="background")` 批量关后台页；⑤ `control(bring_to_front)` 标签页前台化。杀开关 `LAEW_WEB_BACKGROUND_GATE=off`。
- **复用已登录浏览器（connect 模式，`browser_reuse.rs`）**：`open(reuse_existing=true)` 探测本机 CDP 调试端口并 `Browser::connect` 接管保留登录态；探测失败返 **3002**。connect 语义：close 只断连、页面管控恒 open 且零注入。
- **浏览器可见模式与窗口几何**：缺省 = 可见 `headed`；无 GUI 会话自动回退 `hidden`。窗口尺寸规则：headed 导航完成后保持当前窗口尺寸（用户手动调整过以其为准），超屏收边。页面视口铺满窗口内容区。多标签页断流根治：`is_noise_page` 对 new-tab-page 系列不看标题直接判噪声。首启空标签页清扫 + 空白页诊断 + 句柄自愈。

### 多 Agent 架构（8 角色）

| 角色 | 身份 | 职责 | 工具面 |
|------|------|------|--------|
| **Yolo Agent** | `LsmAgentEmergentWork-Yolo` | 入口层：每条输入做 目的→目标→意图 三步分析；任务**三档分类**（simple/medium/hard）；**目标可解析性判定**：填 `target_status`，目标不可解析时编排器直接回问用户（澄清门）不进 WorkFlow；**任务该不该接由模型自行判断**，编排器无任何政策性拒绝门 | `Read`/`Glob`/`Grep`/`Bash`（只读侦察）/ `MCP_Web_Use`（观察类）/ `SubAgent`（只读并行） |
| **Plan Agent** | `LsmAgentEmergentWork-Plan` | 规划层：仅在 hard 任务时启用；输出 Markdown 方案到 `plans/{session_id}-{seq}.md` | `Read`/`Write` |
| **Main-Work Agent** | `LsmAgentEmergentWork-Main-Work` | 流程层：接收 medium/hard 任务，拆 WorkFlow 列表（Kahn 分层 + 同层并行）；编排循环 ReAct 化；复用 LoopGuard 编排层原地打转同样止损 | `Bash`/`Read`/`Glob`/`Grep`/`MCP_Web_Use`（编排前探查）/ `TodoWrite`/`SubAgent` |
| **SubAgent-Work Agent** | `LsmAgentEmergentWork-SubAgent-Work` | 执行层最小单元，每个流程处理单元委派一个 SubAgent；提示词 ReAct 化 + 工具连续工作模式 + 无进展止损 | `Bash`/`Read`/`Write`/`Edit`/`Glob`/`Grep` + `MCP_Window_Use`（仅 macOS/Windows）+ `MCP_Web_Use` + `Use_MCP` + `TodoWrite` |
| **Quality-Check Agent** | `LsmAgentEmergentWork-Quality-Check` | 质检层：每个执行单元完成后必经 QC；**目标一致性硬门**：用户指定 X、实际操作 Y 一律判 Fail + `retryable=false`；**死循环硬门 + 根因裁决通道**：轨迹带 `dead_loop` 诊断时 pass 一律被机械门拒收，并由 `check_dead_loop` 归类根因(`same_action`/`same_target`/`context_stuck`/`budget_starved`)+ 给出可执行换路径,注入单元局部重试 hint | 可选 `Read` |
| **SessionContext Agent** | `LsmAgentEmergentWork-SessionContext` | 会话层：每个用户任务完成后汇总并写入 `session_memory` 表；Yolo 下次处理时自动注入最近 N 条历史摘要 | 无工具 |
| **Debug Agent** | `LsmAgentEmergentWork-Debug` | 调试层：仅在 `-debug` 调试模式下启用；任务结束后对采集的 trace 做评估，产出四章节报告 | 无工具 |
| **Compact Agent** | `LsmAgentEmergentWork-Compact` | 压缩层：上下文估算 token 达到 `context_max_size` 的 80% 时自动触发，按超出幅度自动选三档压缩率，LLM 摘要失败降级本地硬截断；**溢出兜底（reactive）**：真实溢出时自动三级恢复（排水→折叠→暴露） | 无工具 |
| **Reflection Agent** | `LsmAgentEmergentWork-Reflection` | 反思层（Reflection 模式）：执行单元 QC 判 Fail 且将局部重试前做三维度复盘，产出跨 attempt 累积的经验教训注入重试 hint。**反思是顾问不是闸门**（解析/LLM 失败 fail-open）；机械通道零 LLM 兜底；条件触发 + 预算上限 | 无工具 |

#### 跨角色编排（`MultiAgentOrchestrator`）

用户输入 → 项目上下文注入 → Yolo 分类 →（澄清门）→ 简单档（SubAgent）/ 中档（Main→SubAgent）/ 高档（Plan→Main→SubAgent）→ Quality-Check → SessionContext 收口。

- **无程序级拒绝门**：**「这个任务该不该接」完全由模型自行判断**，编排器不做任何检测、拦截或改写。若 Yolo 判断某个任务不适合接，把结论与替代方向写进 `direct_answer`，编排器原样返回用户。
- **同因熔断（`types::SameCauseBreaker`）**：重试环内失败原因归一化连续 2 次相同即提前终止重试。**这是删除程序级拒绝门后唯一的通用空转止动器**。
- WorkFlow 执行时按 `depends_on` 自动 Kahn 分层，**同层无依赖的 SubAgent 自动并行**（tokio::spawn + Semaphore 上限 3），跨层严格串行、上游产物按层注入，失败语义与串行一致（fail-fast 回流 Yolo）。
- **depends_on 净化**：占位符直接丢弃、散文碎片从原文抽取全部已知 wf id、id 形状但未声明保留给拓扑层报精准错。
- **depends_on 缺边推断**：从产物引用反推补边——某单元文本消费了另一样本单元产出的同名文件即补边；逐条加边前做 DFS 成环预检。
- **QC 结论一致性消解**：`verdict=pass` + 非空 issues 属自相矛盾，解析时按 issues 消解为 `Fail`。
- **QC 报告数组字段形状归一化**：模型常把 `notes`/`issues` 输出成内嵌 JSON 数组的字符串，归一化避免整条 JSON 通道作废。
- **执行-验证-修订闭环**：QC 判 `retryable=true` 时先在**单元级局部重试**，预算耗尽才升级到档位级重试 → Yolo 回流 → Failed outcome。
- **Reflection 模式**：条件触发 + 预算上限 + 机械基线 + 杀开关，成功路径零额外 LLM 调用。

### 三个 MCP 风格工具

| 工具 | 能力 | 平台门控 | 设计文档 |
|------|------|----------|----------|
| **MCP_Window_Use** | 桌面窗口操控：`action` 枚举分发（`open`/`list`/`find`/`inspect`/`control`/`ocr`/`screenshot`）；双路线（Windows UIA Pattern 优先 + Win32 消息 + 物理鼠标键盘兜底；macOS AX 控件树 + 物理输入；Linux wmctrl/xdotool 物理层） | **仅 macOS/Windows** 运行时注册；Linux 走物理层兜底 | `docs/MCP_Window_Use/01-设计与解决方案.md` |
| **MCP_Web_Use** | 浏览器操控：`action` 枚举分发（`open`/`list`/`close`/`control`/`inspect`/`sequence`/`batch`/`explore`）；`control_action` 44 个写操作；`inspect` 21 个观察维度；单步 + 连续（`sequence` ≤24 steps）；视口基准 1080p + 2K 自动扩展；蓝色选中边框 + 注入 UI 挂 closed Shadow DOM | 跨 Windows/macOS/Linux；Chrome→Edge→Chromium→Brave 自动探测 | `docs/MCP_Web_Use/01-设计与解决方案.md` |
| **Use_MCP** | 通用 MCP（Model Context Protocol）服务调用统一入口：**真 MCP 协议客户端**（JSON-RPC 2.0），连接外部 MCP server（stdio 子进程 / Streamable HTTP）；单工具 `action` 枚举分发 | 跨平台（由外部 MCP server 决定能力） | `docs/Use_MCP/01-设计与解决方案.md` |

### 自感知动态子 Agent（Self-Awareness SubAgent）

模型自主委派通道：Agent Tools 新增 `SubAgent` 单工具（`action` 枚举 `launch` / `batch` / `list` / `result` / `cancel` / `history` / `resume` / `workflow`）。
**用户提示词显式要求**或任务可分解为 2+ 独立子任务时，Agent 自动组建子 Agent 小队并汇总结果。

- **子 Agent 6 类型**（内置）：`general-purpose` / `explore` / `researcher` / `plan` / `code-reviewer` / `operator`
- **自定义类型**：`.laew/agents/*.md` + `~/.laew/agents/*.md` 两级发现；frontmatter 支持 `label/description/extends/tools/readonly/name`；定义即生效
- **工具面三重收窄**：`tools` 白名单 ∩ `extends` 默认 ∩ `SpawnPolicy` 上限 ∩ 真实注册表
- **运行持久化**：每次运行落 SQLite `subagent_run`；`action="history"` 跨任务/跨会话查询；`action="resume"` 前序任务+结论作为种子上下文注入新子 Agent
- **工作流编排**（`action="workflow"`）：≤8 个 `steps[]` 声明 DAG；Kahn 分层 + 同层并行 + 跨层串行
- **治理**：默认 1 层嵌套；会话级 `Semaphore(3)` + 预算 8 + 单子 Agent 12 轮 / 300s；父 `CancelToken` → `child_token` 级联取消

### Skill 系统（渐进式披露 P0）

- Markdown 文件定义，frontmatter 支持 `name`/`description`/`allowed-tools`/`when-to-use` 等
- 三级加载：元数据始终在 system → 内容按需注入 → 完整文件按需 Read
- 内置 skill：`code-review` / `git-commit` / `test-runner`（位于 `src/agent/skills/bundled/`）
- 用户级 + 项目级两级覆盖（`.laew/skills/*.md` + `~/.laew/skills/*.md`）

### Agent-Context / Agent-Memory / SessionContext

- **Agent-Context**：每个 Agent 独立的实时上下文（消息流 + 状态），内存态，生命周期 = 当前单元
- **Agent-Memory**：每个 Agent 独立的记忆层，持久化到 SQLite `agent_memory` 表，跨单元/跨 Session 复用
- **SessionContext 摘要**：每个用户任务完成后 SessionContext 生成 Markdown 摘要写入 `session_memory` 表；Yolo 下次处理时自动注入最近 N 条历史摘要（默认 3），用 `<<<LAEW:SESSION_HISTORY>>>` 标记隔离

### AgentProfile / Session / 请求头 / 系统提示词三段式

- **AgentProfile**：Agent 身份档案（名称 / 系统提示词 / 工具集 / `spawn_policy`），`work_profile` / `yolo_profile` / `dynamic_child` 工厂函数
- **Session**：进程内会话，拥有独立 Session ID 与对话上下文（context）；TUI 启动或 `/new` `/clear` 时生成新 Session
- **会话持久化**：TUI 每轮任务收口把 transcript **整快照重写**到根目录 SQLite `chat_sessions`/`chat_turns`；自动保留最近 50 个会话。恢复走 `/sessions` `/resume` / `--resume`
- **请求头**：两协议统一携带 `User-Agent: {AgentName}/{版本} {编译时间}`、`Authorization: Bearer {api_key}`、`X-Session-Id`；Anthropic 请求体 additionally 携带 `metadata.user_id`
- **系统提示词编写原则（9 角色统一）**：所有角色提示词按两类约束切分——**政策性约束不写进提示词，程序也不拦截**（判断权完全交给模型）；**运行正确性约束保留**（各有运行时闸门配套）。**刚性三档**：硬（必须）/ 中（默认如此，例外需理由）/ 软（建议）。**两条硬规矩**：① 提示词正文不写「第 N 轮」沿革标注；② 入口层角色的分层约定写成「默认如此 + 例外需理由」而非禁令。
- **Anthropic 三段式系统提示词**：8 角色 + WorkFlow 角色的 `system` 字段从单字符串重构为三段式（billing 计费头 + identity 基础身份声明 + rules 核心行为规则）。**runtime hints 不进 system**：逐轮变化的进度/收口/无进展提醒经 `RequestMeta.runtime_tail` 由 wire 层拼到消息流末尾尾注。

## 架构（src/）

```
main.rs            clap CLI 入口（默认进 TUI；-p 单轮；-f 文件提示词；provider / mcp 子命令）
lib.rs             库导出
artifact_root.rs   产物根目录唯一真源
session.rs         Session:本机指纹 device_id + Session ID + 独立对话上下文 context
error.rs           AgentError(含 YoloParse)
shutdown.rs        graceful shutdown(TUI 退出时清浏览器子进程,避免双重 panic)
crash.rs           CrashDump:panic hook + 信号恢复 + heap dump 自动触发
frontmatter.rs     Markdown frontmatter 解析(自定义斜杠命令 + 自定义子 Agent 类型共用)
logging.rs         tracing 双层订阅器:控制台层行为不变 + 文件层运行日志(llaew_*.log)
test_support.rs    单元测试公共夹具

tui/
  mod.rs           会话外壳:TuiSession 结构 + 生命周期 + orchestrator 装配 + run/run_with_debug 入口
  dispatch.rs      输入分发与任务输出
  slash.rs         斜杠命令路由
  provider_screen.rs  /provider 子屏桥接
  format.rs        纯函数格式化
  format_brief.rs   [tool]/[laew] 行关键字段摘要
  audit_view.rs    /audit 决策审计可视化面板
  banner.rs        启动横幅
  engine.rs        CLI 渲染引擎 —— Screen trait + Frame + 全量重绘 present
  form.rs          通用 Tab 表单状态机
  hitl_view.rs     人工介入(HITL)TUI 呈现视图
  input.rs         单行输入 + 自适应底部面板
  paste.rs         粘贴保真层
  completion.rs    斜杠命令补全引擎
  commands.rs      自定义斜杠命令
  export.rs        会话导出
  branches.rs      对话分支存储
  mention.rs       @ 文件提及解析 + 实时路径补全
  pathfmt.rs       路径格式化辅助
  textfit.rs       显示宽度唯一真源
  theme.rs         ANSI 颜色 / mask_key 脱敏
  term_guard.rs    终端守卫
  screen/
    provider_list.rs / provider_form.rs / provider_del.rs

agent/
  mod.rs           agent 域模块注册表 + Agent 结构体/构造器/访问器
  agent_loop.rs    Agent 核心循环
  agent_message.rs Agent 消息模型
  attachments.rs   @ 文件提及附件注入
  tool_exec.rs     工具调用执行底座
  loop_guard.rs    ReAct 循环守卫(工具级无进展)
  dead_loop/       死循环守卫(LLM 交互级指纹检测:小循环/大循环)
  runtime_hints.rs Agent 循环运行时辅助
  context.rs       Agent-Context 独立实时上下文
  extrace.rs       提取/抽取工具输出关键信息的助手
  cancel.rs        取消竞争
  react_tests.rs   ReAct 行为单测

  profile.rs       AgentProfile + work/yolo/dynamic_child 工厂函数
  custom_agents.rs 自定义子 Agent 类型
  self_awareness.rs 自感知层
  dynamic_subagent.rs 动态子 Agent 运行时
  subagent.rs      SubAgent 主逻辑
  subagent_workflow.rs SubAgent 工作流编排
  decision_audit.rs 决策审计
  human_assist.rs  人工介入 HITL 枢纽
  human_ui/        人工介入弹窗 UI 呈现层
  todo_state.rs    TODO 任务状态
  memory.rs        Agent-Memory SQLite 持久化
  max_tokens_state.rs max_tokens 三级恢复状态机
  project_context.rs 项目说明文件五级链发现 + 注入
  session_context.rs SessionContext Agent
  session_fork.rs  对话 Rewind 轮次扫描
  workspace.rs     工作区感知
  offline_queue.rs 离线模式

  json_repair.rs   JSON 自动修复链
  partial_json.rs  partial JSON 解析
  overflow.rs      上下文溢出检测 + 三级恢复
  plan.rs          Plan Agent
  plan_validate.rs Plan 输出校验
  yolo.rs          YoloRunner 双 Agent 编排器
  main_work/       Main-Work 流程层目录
  orchestrator/    MultiAgentOrchestrator 总编排器目录
  quality/         Quality-Check Agent(mod.rs 结构体+构造器+提示词构建;dead_loop.rs 死循环根因裁决通道;tests.rs)
  debug.rs         Debug Agent
  compact.rs       CompactRunner
  subagent_workflow.rs SubAgent 工作流编排

  permissions/     权限管控
  safety/          工具层防护
  sandbox_hook/    沙箱钩子
  skills/          Skill 系统
  system_prompt/   SystemPrompt 组合与渲染
  tools/
    mod.rs         Tool trait + ToolRegistry
    bash.rs        BashTool
    read.rs        ReadTool
    write.rs       WriteTool
    edit.rs        EditTool
    glob.rs        GlobTool
    grep.rs        GrepTool
    emit.rs        EmitTool
    todo.rs        TodoWrite
    subagent.rs    SubAgentTool
    read_detect.rs ReadTool 二进制/编码自动探测
    mcp_window_use/  MCP_Window_Use 工具目录
    mcp_web_use/     MCP_Web_Use 工具目录
    use_mcp/         Use_MCP 工具目录

  window/          窗口操控平台驱动层
  browser.rs       浏览器 CDP 驱动层
  browser_mode.rs  浏览器启动模式决策唯一真源
  browser_noise.rs   幽灵标签页判定
  browser_follow.rs  自动跟随滚动
  browser_overlay.rs 页面管控模块
  browser_status.rs 屏蔽页面状态栏/进度栏模块
  browser_spawn_arm.rs 派生页首文档早期武装
  browser_env_emulation.rs 无头档指针/悬停媒体仿真
  cdp_ext.rs       chromiumoxide 0.9 类型化 CDP 命令补丁层
  display_topology.rs OS 显示器拓扑真值
  fingerprint.rs   反检测指纹体检
  web_activity.rs Agent 活动台账
  browser_viewport.rs 窗口与视口尺寸决策
  browser_watchdog.rs Browser 子进程 watchdog
  workflow/        Goal 状态机 + Squad 调度工作流层
  workflow_json_validate.rs Workflow JSON 校验

llm/
  mod.rs           统一消息模型 + LlmClient trait + RequestMeta + build_common_headers + build_http_client
  anthropic.rs     Anthropic wire 转换
  openai.rs        OpenAI wire 转换
  sse.rs           SSE 流式响应解析
  cancellable.rs   可取消 LLM 调用
  resilient.rs     LLM 调用自动弹性层
  cache_policy.rs  PromptCaching 策略
  offline.rs       离线模式 LLM 调用降级
  pricing.rs       Token 单价表

mcp/               通用 MCP 客户端层

database/          SQLite 全栈基础设施

config/            全局配置
```

## TUI 界面（独立 CLI 渲染引擎）

### 屏幕拓扑

- **自适应底部面板**：输入面板不再恒定吸屏幕底部，而是**顶行紧跟当前光标行**。滚动区底行改为 `panel_top`。**坑**：查光标行必须**在 `ESC[r`(DECRST)之前**发 `ESC[6n`。`crossterm::cursor::position` 本机恒返回 `Ok((0,0))`，不可用，改用自实现 `query_cursor_row`。
- **人工介入 `/hitl` 应急通道**：弹窗接管期间 TUI 此前完全不读 stdin，弹窗一旦屏外/被遮挡/进程卡住就是 120s 死等。现并联一条**只认 `/hitl` 前缀**的行读。
- **REPL 主屏**：保留 `InputHandler` 单行输入 + 斜杠命令补全 + 多轮对话。**粘贴保真**：bracketed paste 整体接收；≥2 行粘贴一律转 marker 保真。
- **子屏（Modal）**：`engine.rs` 的 Screen 栈接管 `/provider *` 系列，进入 alternate screen + 原始模式，Esc 退回主屏。
- **非 TTY 回退**：stdin 不是终端时（管道 / 重定向），子屏与主屏都回退到 print 输出。

### 斜杠命令

| 命令 | 行为 |
| ---- | ---- |
| `/help` (h, ?) | 显示帮助 |
| `/exit` (quit, q) | 退出 TUI |
| `/clear` (c) | 清空对话历史，开启新 Session |
| `/new` (n) | 同 `/clear` |
| `/model` | 显示当前模型 |
| `/rewind [N]` | 对话回退（D3） |
| `/undo` | 撤销最后一轮对话 |
| `/fork` | 从当前对话分叉出新 Session |
| `/branches` | 列出已存分支 |
| `/switch <name>` | 切换到指定分支 |
| `/sessions` (`hist`) | 列出跨进程可恢复的历史会话 |
| `/resume [N\|id前缀]` | 恢复历史会话 |
| `/offline` (`status`) | 查看连接状态与离线队列深度 |
| `/cost` (`usage`) | 查看会话用量与成本估算 |
| `/workspace` (`ws`) | 查看工作区快照 |
| `/export [path]` | 导出当前会话 |
| `/tasks` (`todo`, `todos`) | 列出当前 session 的 TODO 任务清单 |
| `/agents` (`subagents`) | 自感知动态子 Agent 面板 |
| `/audit` (`audits`) | 决策审计可视化 |
| `/risk` (`risks`) | 站点反爬熔断状态面板 |
| `/commands` | 列出已加载的自定义斜杠命令 |
| `/provider` | 管理接入记录 |

### 自定义斜杠命令（D2）

Markdown Prompt 模板，两级发现：**项目级** `{工作目录}/.laew/commands/*.md` + **用户级** `~/.laew/commands/*.md`（同名用户级优先）。frontmatter 支持 `description` / `argument-hint`。实现见 `src/tui/commands.rs`。

### 关键约定

- 新增子屏：实现 `engine::Screen` trait，在 `mod.rs::handle_slash` 中路由。
- 新增**盒线/信息块**：一律走 `tui::textfit::InfoBox`，禁止硬编码边框长度。
- 不引入新 crate（crossterm 已满足）。
- 子屏不直接写 stdout；通过 `Frame` → `engine::present` 统一绘制。

## 文档地图（docs/）

**核心设计文档**（按模块）：
- `docs/多Agent架构重构/` — 从 0 到 1 的分阶段解决方案
- `docs/TUI界面与CLI渲染引擎/` — 独立 CLI 渲染引擎 + Tab 表单
- `docs/TUI交互优化与-f命令设计.md`
- `docs/TUI输入处理常见陷阱与修复记录.md`
- `docs/TUIMarkdown富文本渲染/`

**Agent 设计**：
- `docs/YoloAgent设计/` — Yolo 入口层设计
- `docs/PlanAgent` / `docs/Main-Work工具扩展与ReAct改造/`
- `docs/SubAgentWork执行层ReAct与连续工作模式/`
- `docs/Quality-Check Agent` / `docs/SessionContext Agent` / `docs/Debug模式与DebugAgent设计/`

**协议与请求层**：
- `docs/Anthropic协议系统提示词三段式/`
- `docs/协议抓包/`
- `docs/其他Agent工具定义/`

**三个 MCP 风格工具**（按工具）：
- `docs/MCP_Window_Use/` — 桌面窗口操控
- `docs/MCP_Web_Use/` — 浏览器操控
- `docs/Use_MCP/` — 通用 MCP 服务调用

**上下文与会话**：
- `docs/Context设置与自动压缩设计/`
- `docs/Yolo项目上下文注入/`
- `docs/工作区感知与运行时环境注入/`

**自感知 SubAgent 三件套**：
- `docs/自感知SubAgent动态启动/` — D114 基础委派
- `docs/自感知SubAgent自定义类型与运行持久化/` — D115
- `docs/自感知SubAgent工作流编排/` — D116

**守卫与止损**：
- `docs/LLM交互死循环指纹检测与质检裁决/` — LLM 交互级死循环指纹检测 + QC 根因裁决
- `docs/Reflection反思模式/`

**Skill / HITL / 离线 / 审计**：
- `docs/Skill系统与渐进式披露/`
- `docs/多轮对话问题知识库/`
- `docs/TODO任务清单/`

**数据库与基础设施**：
- `docs/数据库重构与Provider导入导出/`

**测试与构建**：
- `docs/自动化测试-提示词文件列表/`
- `docs/自动化测试与Debug一体化/`

**调研归档**：
- `docs/Agent源码调研/`
- `docs/Agent架构对比与参考.md`
- `docs/工程初始化方案/`

## 自动化测试

**全部测试都是 Rust 代码级测试，统一用 `cargo test` 跑。**

| 层 | 入口 | 用途 |
| --- | ---- | ---- |
| 单元测试 | `cargo test --lib` | Rust 函数级覆盖 |
| 集成测试 | `cargo test --test <name>` | 跨模块的功能测试 |

### 集成测试布局（`tests/`）

| 文件 | 覆盖 |
| ---- | ---- |
| `tests/support/mod.rs` | **公共夹具**：进程内 mock LLM 服务（真实 HTTP/SSE） |
| `tests/protocol_wire.rs` | 双协议 wire 契约 |
| `tests/agent_loop.rs` | Agent 主循环功能 |
| `tests/cli_provider.rs` | 真实二进制 CLI：`provider` / `mcp` 增删列切 |
| `tests/project_context.rs` | 项目说明文件五级发现链 + 注入幂等 |
| `tests/web_use_smoke.rs` | MCP_Web_Use 真浏览器冒烟（默认 `#[ignore]`） |
| `tests/stealth_reveal_smoke.rs` | 反检测的服务端侧取证 + 回填替身真机差分 |
| `tests/macos_ax_smoke.rs` | macOS Accessibility 冒烟（依赖宿主 TCC 授权） |
| `tests/browser_watchdog.rs` | 浏览器子进程回收 |

### 写集成测试的规矩

1. **不打桩生产类型**。mock 只决定「模型这一轮说什么」。
2. **一律用绝对路径**。进程 cwd 是全局量，集成测试默认并行。
3. **CLI 用例先复制二进制到 `tempfile` 临时目录**再执行。
4. **mock 端口交 OS 分配**（`127.0.0.1:0`），禁止固定端口。
5. **中文测试函数名**，与本仓库单测风格一致。
6. **断言失败要能自解释**。

### 单测与集成测试的分工

能在函数级判定的写 `src/**` 内的 `#[cfg(test)] mod tests`；
只有「跨模块串起来才有意义」的才写 `tests/`。

### TUI 子屏仍需 tmux（当前无自动化）

`/provider list` 等子屏行为**无法用 `cargo test` 覆盖**，需人工或临时 tmux 验证。

## 约定

- 注释、CLI 文案、文档一律中文；代码标识符英文。
- **单源码文件 ≤ 1800 行**：超过必须按功能/业务/架构维度拆分模块。**拆分规范**：
  - 单文件超线 → 转同名目录（`xxx.rs` → `xxx/mod.rs` + 职责子模块）；`mod.rs` 承载结构体定义 + 构造器/入口 + `pub use` 再导出，**外部 `crate::…::xxx::Yyy` 路径零改动**；
  - 原私有项搬入子模块标 `pub(super)`，由 `mod.rs` 私有 `use` 重导入供兄弟模块 `use super::*` 取用；
  - 代码**逐行机械搬移零改写**（不重构逻辑/注释），仅新增模块文档头与 `use super::*`；测试搬 `tests.rs`（mod.rs 声明 `#[cfg(test)] mod tests;`），拆分前后测试数必须对账一致；
  - 测试模块一律用 `xxx/tests.rs` **文件**；某个 `tests.rs` 自身超线时，把新用例放到**同级兄弟文件**，不要转目录；
  - 高速增长文件（近几轮每轮 +50 行以上）在破线前预防性拆分。
- 新工具：在 `src/agent/tools/` 建同名模块实现 `Tool` trait，注册进 `builtin_registry` 或相应 registry。
- 新协议：实现 `LlmClient` trait + `client_from_record` 增加分支。
- 新 Agent 类型：实现 `AgentProfile`，在 `YoloRunner` 或相应编排器中接入。
- 临时计划放 `tmpPlan/`（已 gitignore）。测试产物同样不入库。
- `laew`、`*.db`、`tmpPlan/`、`target/` 均不入库（见 .gitignore）。

## Git 提交规范

**格式**：`<type>(<scope>): <中文业务描述>`，沿用仓库既有风格。
`type` 取 `feat` / `fix` / `perf` / `refactor` / `test` / `docs` / `chore`；`scope` 用模块名。

**硬性禁止**：
1. 禁止添加任何 `Co-Authored-By: Claude Code <noreply@anthropic.com>` 行。
2. 禁止追加 `Generated with Claude Code` 这类 AI 署名/生成声明文本。
3. commit message **只保留业务描述**，末尾不追加任何 AI 相关的 attribution 尾部注释。

**其它约定**：
- 默认**不主动提交**：改完代码先向用户汇报 diff 摘要，等明确指示再 `git commit`。
- 需要提交时先确认在 `main` 之外的分支（默认分支上先建分支）。
- 提交前确保 `cargo build` 通过；改动范围大时同步跑 `cargo test`。
