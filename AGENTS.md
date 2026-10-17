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
| | 未设置（默认） | 自动模式：endpoint 主机为 IP（IPv4/IPv6）时自动跳过证书校验，域名主机仍严格校验。适配 IP + 自签名证书的内网/自建 HTTPS 网关；仅跳过校验，TLS 加密不降级；rustls 纯 Rust 实现，Windows/macOS/CentOS/Ubuntu 行为一致。设计见 `docs/自签名证书TLS适配/01-设计与解决方案.md` |
| `LAEW_ALLOW_PRIVATE_ENDPOINT` | `1` | SSRF 防护放行私网/loopback endpoint（本地 Ollama / 局域网 / mock 测试 provider 用；默认拦截，见 `src/agent/safety/url_safety.rs`） |
| `LAEW_BASH_UTF8` | `1`/`true`/`yes`/`on` | Bash 工具为子进程注入 UTF-8 环境（`PYTHONUTF8=1`/`PYTHONIOENCODING=utf-8`/`LC_ALL=C.UTF-8`），消除 Windows 区域设置(GBK)导致的 python/coreutils 输出乱码；默认关闭。行尾(CRLF)不受影响，精确 diff 场景脚本仍需 `reconfigure(newline=...)`，见 `src/agent/tools/bash.rs` |
| `LAEW_ARTIFACT_DIR` | 任意路径 | 覆盖**产物根目录**（缺省 = 启动时工作目录 > 程序目录）。八类产物 `plans/`/`DebugReport/`/`AuditTrail/`/`BashSpill/`/`logs/`/`CrashReport/`/`EvalSpill/`/`WebShots/` 全落这里，不再逃逸到二进制所在目录（实测经 `/opt/homebrew/bin` 软链启动时产物全落进包管理目录）。见 `src/artifact_root.rs` |
| `LAEW_AMBIGUOUS_WIDE` | `1`/`true`/`yes`/`on` | 歧义宽度字符（`✓ · … → ║` 等 East Asian Ambiguous）按 **2 列**参与宽度计算（默认关闭=按 1 列）。只改计算不改渲染，供把盒线渲染成双宽的老终端（部分 Windows conhost 中文字体 / xterm `-ctwidth`）校正对齐；见 `src/tui/textfit.rs::ambiguous_wide` |
| `LAEW_LOG_CLIP` | 正整数（默认 `4000`） | `--debug`/`--info` 运行日志文件中单字段（LLM 思考文本/工具参数/结果等）的截断长度（字符数）；`0`/非法值回退默认。见 `src/logging.rs` |
| `LAEW_AUDIT` | `off`/`0`/`false`/`no` | 关闭决策审计写入（默认开启）。开启时 5 个决策点（Yolo 分类 / Plan 规划 / Main-Work 拆解 / QC 判定 / Compact 压缩）各追加一条结构化 JSON 行到根目录 `AuditTrail/audit_{session_id}.jsonl`（已 gitignore），记录「输入上下文→决策结论→决策依据」三段式 + 耗时/token/扩展字段，全字段脱敏截断，fail-open 不影响主流程。见 `src/agent/decision_audit.rs` |
| `LAEW_PARALLEL_TOOLS` | `off`/`0`/`false`/`no` | 关闭「同批只读工具并发执行」，回退到全串行。默认开启：同一 LLM 响应里**连续的** `parallel_safe` 调用（Read/Glob/Grep，段长 ≥2）并发执行，`tool_result` 仍按原序回填。见 `src/agent/tool_exec.rs` |
| `LAEW_MAX_PARALLEL_TOOLS` | 正整数（默认 `4`） | 单批并发上限；`0`/非法值回退默认。对齐 AtomCode `ATOMCODE_MAX_PARALLEL_TOOLS=4`（Claude Code 用 10，laew 单元迭代预算小取保守值）。见 `src/agent/tools/mod.rs::max_parallel_tools_from` |
| `LAEW_REACT_GUARD` | `off`/`0`/`false`/`no` | 关闭 ReAct 循环守卫（无进展软提醒与 doom_loop 止损均不生效；`doom_loop_repeats` 仍统计，可观测性不丢）。默认开启：进展键 =「工具名 + 参数稳定 JSON + 结果摘要」三者全同即无进展，第 2 次软提醒 / 第 3 次宽限轮强提醒 / 第 4 次止损终止。见 `src/agent/loop_guard.rs` |
| `LAEW_MAINWORK_TIMEOUT` | 正整数(秒,默认 `300`) | Main-Work 拆解(整个 ReAct 多轮会话)的墙钟超时;超时按 retryable 回流重试。第 160 轮由 120s 提到 300s:慢网关单次 LLM 调用实测 60~201s,120s 连续误伤导致同因熔断收 failed。挂起兜底靠同因熔断与 LLM 弹性层。见 `src/agent/orchestrator/pipeline.rs` |
| `LAEW_TOOL_TIMEOUT_SECS` | 正整数(秒,默认 `300`) | **单条工具调用总执行超时**(最后防线,2026-10-16):任何底层无界 await(实测:大陆网络下 `github.com/commits` 导航 stall 卡死 chromiumoxide handler → 后续 CDP 调用永久排队)命中预算即返回结构化错误(`AgentError::ToolExecution` 带恢复指引),Agent 循环继续而非无声停摆。豁免:`Bash`(自带 120s 默认/600s 上限进程级超时)与 `MCP_Web_Use` 的 `request_human`(HITL hub 30 分钟上限);`0` = 关闭回退旧行为。配套:高扇出 CDP 点(`list_pages`/`close_blank_pages`/`close_page`/`new_page`/open 的 `get_title`/`url`)统一走 `src/agent/browser_cdp.rs::cdp_with_timeout` 有界化(默认 10s,fail-open),`list_pages`/`close_blank_pages` 改快照-探测-三段式不再持全局锁跨 await;open 复用路径导航失败先剔除死页再走新建。见 `tmpPlan/20261016_163802_工具执行总超时与CDP有界化根治浏览器挂死.md` |
| `LAEW_WF_SERIAL_FALLBACK` | `off`/`0`/`false`/`no` | 关闭**零依赖计划串行链兜底**(默认开启)。`sanitize_depends_on`+`infer_depends_on` 之后所有单元 `depends_on` 仍全空且单元数 ≥2 时,按声明顺序补 `wf[i]→wf[i-1]` 链边(全空=无并行证据,乱序互踩风险>并行收益)。背景:第 165 轮 Plan 文档全角冒号使依赖行被行级解析静默丢弃,5 单元同层并发——wf-5(生成文档)早于 wf-4(提取数据)执行产出编造内容,上游局部重试又与在飞单元并发互踩同一浏览器页面,重试风暴 17 分钟直到人工取消;同轮已修全角标点兼容(见 `src/agent/main_work/parse.rs`)。真独立单元可并行的场景可关本兜底 |
| `LAEW_REPLAN` | `off`/`0`/`false`/`no` | 关闭**Plan 再规划/更新规划**(默认开启,每任务预算 `OrchestratorConfig::replan_budget`=2)。执行层按方案执行遭遇规划缺口(登录墙/验证码/表单填报/访问受阻/现场与规划假设不符,`src/agent/replan.rs::detect_replan_signal` 机械识别)时,编排器把失败现场回灌规划层修订方案:hard 档 `PlanRunner::replan_update`(原方案+失败现场+已完成清单→新方案文档,跳过计划级 QC),medium 档 Main-Work 带 `replan_context` 更新编排(须过确定性校验);`ReplanPending` 跨轮暂存,下一轮 run_hard/run_medium 直接消费新方案跳过重新生成,执行单元经档位级 retry_hint 看到「规划已如何更新」。再规划失败静默回落常规重试;`failure.retryable=false`(如 target_drift)不再规划。设计见 `docs/PlanAgent再规划与更新规划/01-设计与解决方案.md` |
| `LAEW_DEBUG_SYNC` | `1`/`true`/`yes`/`on` | 恢复 `--debug` TUI **同步**生成调试报告(默认后台化):任务收口后 `finalize_report`(Debug Agent LLM 评估,真实网关实测 155s/194s)tokio::spawn 后台执行,完成经 oneshot 回传、主循环 read_line 前收割打印;退出时有界等待 20s。设置本变量恢复第 132 轮旧同步行为(测试/确定性场景)。见 `src/tui/dispatch.rs::spawn_debug_report` |
| `LAEW_REFLECTION` | `off`/`0`/`false`/`no` | 关闭 **Reflection（反思）模式**（默认开启，含机械轻量通道）。开启时执行单元 QC 判 Fail 且将局部重试前做三维度复盘（逻辑自洽/事实准确/目标匹配），产出跨 attempt 累积的经验教训注入重试 hint；机械通道（`ExecutionTrace` 结构化信号→通用教训）零 LLM 始终可作基线。反思是**顾问不是闸门**（解析/LLM 失败 fail-open 落回机械教训）。见 `src/agent/reflection.rs`、`docs/Reflection反思模式/01-设计与解决方案.md` |
| `LAEW_REFLECTION_MODE` | `deep`（默认）/ `light` | 反思档位：`deep` = 条件触发 LLM 深度反思 + 机械教训合并（只在「即将带着错误重跑」时调 LLM，成功路径零开销）；`light` = 全程零 LLM 只跑机械教训（超低延迟/离线/测试场景） |
| `LAEW_REFLECTION_BUDGET` | 正整数（默认 `2`） | 每单元深度反思次数上限（与 `unit_retry_budget` 对齐）；超限退化为纯机械教训，不再调 LLM。`0`/非法值回退默认 |
| `LAEW_MCP_ENABLED` | `off`/`0`/`false`/`no` | 关闭通用 MCP 服务调用（`Use_MCP` 工具注册与提示词同时归零，严格向后兼容）。默认开启：Agent 可经 `Use_MCP` 调用 `laew mcp add` 配置的外部 MCP server 工具/资源。见 `src/agent/tools/use_mcp/mod.rs` |
| `LAEW_SELF_SPAWN` | `off`/`0`/`false`/`no` | 关闭**自感知动态子 Agent**（默认开启）。关闭时不注册 `SubAgent` 工具、不注入自感知提示词段、不建运行时（工具面/提示词/耗时与改造前完全一致）。见 `src/agent/self_awareness.rs` |
| `LAEW_TARGET_ANCHOR` | `off`/`0`/`false`/`no` | 关闭**任务锚点**全部四层（L1 澄清门 / L2 伪澄清单元阻断 / L3 工具门 6001 / L4 目标一致性 QC 门 + `target_drift` 信号）。默认开启，根治「多行粘贴被截断后 Agent 自行改派到无关站点仍判成功」事故。详见「领域概念 · 任务锚点」 |
| `LAEW_TARGET_ANCHOR_BLOCK` | `off`/`0`/`false`/`no` | 仅关闭 L3 工具级**阻断**（`MCP_Web_Use` 的 6001 不返回，仅记 warn），保留 L1 / L2 / L4 与信号打标（观察模式）。默认开启阻断。适合灰度期先观察不拦截 |
| `LAEW_BROWSER_MODE` | `headed`/`hidden`/`new_headless`（大小写不敏感，别名 `head`/`with_head`/`visible`/`inprocess`/`cdp_only`/`headless`；旧 bool `false`/`0`/`off` ≡ `headed`，`true`/`1` ≡ `new_headless`） | 浏览器启动模式**显式覆盖**。不设时按 `has_gui_session()` 判定：有 GUI → **缺省可见 `headed`**；无 GUI（`CI`/`SSH_*`/Linux 无 `DISPLAY`·`WAYLAND_DISPLAY`/macOS 非 `Aqua`）→ 回退 `hidden`。`LAEW_FORCE_HEADLESS=1` 强制无头。见 `src/agent/browser_mode.rs` |
| `LAEW_WEB_EVIDENCE` | `off`/`0`/`false`/`no` | 关闭**网页取证纪律**硬闸门（默认开启）。命中「Bash 网络取证命令 + 命令中的 host 落在任务锚点域内 + 工具面含 MCP_Web_Use」时直接拒绝并推回 MCP_Web_Use；`curl localhost:8080` 等非任务目标一律放行。见 `src/agent/safety/web_evidence.rs`、`docs/MCP_Web_Use/04-网页取证纪律.md` |
| `LAEW_WEB_FINGERPRINT` | `off`/`0`/`false`/`no` | 关闭**环境指纹体检与保真回填**（默认开启，legacy 杀开关；新代码用 `LAEW_WEB_STEALTH`）。开启时 `open` 响应附带 `data.fingerprint{suspicious:[{code,advice}]}`（WebGL 兜底串 / webdriver 漏压 / 插件或语言为空 / chrome.runtime 缺失 / 无头 UA 令牌 / 描述符被改 / 函数身份可见），且 `browser_ns::FIDELITY_JS` 仅在页面读数是**自动化兜底串或空值**时回填为与本机一致的值（纯指纹保真，不改渲染、不碰验证码与风控判定）。关闭后 `webdriver` 核心覆写仍在（只关回填与体检）。见 `src/agent/fingerprint.rs`、`src/agent/stealth.rs`、`docs/MCP_Web_Use/26`；**第 179 轮自证项扩充**：`injected_ns_keys`（应恒 0，非零即命中 `injected_ns_string_keys`）/ `injected_ns_symbols`（应 1）/ `ns_symbol_present`（命名空间对象在场）/ `outer_equals_inner`（无头档窗口几何软提示）/ `patched_fn_leak` 补 `languages`·`pdfViewerEnabled` getter 与 `Function.prototype.toString` |
| `LAEW_WEB_STEALTH` | `hardened`（缺省，2026-10-16 升档）/`fidelity`/`off`（别名 `非侵入式`/`侵入式`/`保真`/`原生化`/`compatible`/`intrusive`，大小写不敏感） | **反检测档位**（第 174 轮，`src/agent/stealth.rs`，`docs/MCP_Web_Use/26` + `31`）。**侵入式 `hardened`（缺省）** = 保真档全部 + `Function.prototype.toString` 原生化（含自反闭合：`toString.toString()` 亦报 `[native code]`），让「我们改过」本身不可被反查。缺省从 fidelity 升档依据（2026-10-16 BOSS 直聘实测）：fidelity 档下被改写函数 toString 源码可读（`patched_functions_visible`），geetest 类风控 SDK 采集该信号加权，匿名访问首页即被 302 到 verify.html；hardened 下该签名从根上消失。**非侵入式 `fidelity`** = 只移除自动化泄漏 + 回填本机真值：webdriver 覆写、WebGL UNMASKED_* 兜底串回填、空 plugins/languages/chrome.runtime 回填、Notification.permission 与 permissions.query 一致性、被改写原生函数的描述符形态还原（enumerable，否则 `for…in` 枚举原型链即暴露）、无头档 UA 的 `HeadlessChrome` 令牌清理（经 `Emulation.setUserAgentOverride` 同时改 UA 串/网络头/Sec-CH-UA 品牌表，取值全部来自页面自报值，只滤品牌，零伪造）；`LAEW_WEB_STEALTH=fidelity` 显式降档。`off` = 零处置（排障对照）。工具面 `open(stealth=…)` / `control(set_stealth,mode=…)` 运行时覆盖（全局作用于全部存活页面；升档即时生效，降档只对新文档生效）；同档重复注入由幂等闸拦住（零叠加，否则包装器会套两层把原生化判成非 native）；**第 177 轮**幂等闸升级为**块位掩码**（`core=1/fidelity=2/hardened=4`，`window.<ns>StealthRev` 存已装块位，`__need = want & ~cur` 只补跑缺位块），跨档升级不再整脚本重跑、不叠加包装；身份登记表 `__fd` 是闭包常量（挂 window 会新增 `getOwnPropertyNames` 可见的全局键，本身即签名），故跨档时靠原生化前的 `__adoptAll()` **重新认领**上一轮改写（八个固定入口 + 回填替身各层中间原型），迭代器的 `next`/自反方法每次调用现造、原型无引用可认领 → 改用 `__mkIterator` + `__rehardenIterator` **重装**（items 由原生 `item()` 反查；只重装自造中间原型，真原型一律跳过，避免在真实插件环境里改坏原生迭代器）。**隐身脚本必须在 `page.goto` 之前注册**（`addScriptToEvaluateOnNewDocument` 只有导航前下发才有 document-start 语义；排在 goto 之后会让首个文档全程裸奔——风控 SDK 恰在文档最早期采指纹并上报，表现为「首航必被拦、刷新反而好转」）；**第 178 轮**建页改用 **`about:blank` 起页**（此前把目标 URL 交给 `Target.createTarget`，而 chromiumoxide 会等它**加载完成**才返回 → 首个真实文档跑在处置之前，随后 `goto` 又把同一 URL 再加载一次；真机计数服务器实测目标 URL 被请求 **2 次**、`req#1` 的 UA 原样带 `HeadlessChrome`；现在收敛成**唯一一次**且自始在处置之下。`about:blank` 非安全上下文读不到 Client Hints，而 `setUserAgentOverride` 不带 `userAgentMetadata` 会把页面侧品牌表**清空**，故 UA 去令牌改**二段式**：导航前在 about:blank 上只清 UA 字符串（唯一一次真实请求不带令牌），落地后在真实安全上下文 `restore_native_ua()` 撤掉半吊子覆盖 → 重新探到品牌表真值 → 下发完整覆盖；有头档此路径不可达）。connect 模式（接管用户浏览器）零注入契约不变。**明确不做**：Canvas/字体指纹伪造、行为轨迹仿真、IP 代理池、打码平台、协议逆向（理由见 docs §三）；**亦不做** Worker 构造拦截（真机实测 worker 内 `navigator.webdriver` 为 `undefined`、`navigator.plugins` 不存在，裸环境读数相同，非我们的泄漏）与 `Runtime.enable` 处置（Chrome 155 真机四路径检测全部不可复现，处置需 fork CDP 客户端，见 `docs/MCP_Web_Use/32` §五）；**第 179 轮：注入面收敛**（`docs/MCP_Web_Use/33`）—— 窗口面注入 API 从 **30 个 `window.<ns>Foo` 字符串键**收敛到**单一 `window[Symbol.for(<ns>)]` 命名空间对象**（`injected_ns_keys` 30→0、`injected_ns_symbols` 0→1，命名空间内部键统一 `enumerable:false`），DOM 面 id/class/`data-*` 换用**独立的第二随机 token**（`browser_ns::dom_ns`，不再复用 `__lzn` 前缀）；渲染期 `render_ns` 按后缀白名单（长优先）把 `window.<ns>X` 改写成 `NS.X`，Rust 侧 eval 走 `ns_member_expr` |
| `LAEW_READ_IMAGE_B64` | `1`/`true`/`yes`/`on` | 恢复 Read 工具**图片 base64 全文返回**（缺省为存根：元信息 + 指纹前 96 字符 + 视觉判读指引）。背景：wire 层从未把 base64 标记块转成协议级 image content block，纯文本注入既看不见图又付出巨额 prefill（实测 QC 一次读两张截图 → 单会话 80 万 input token、106s 延迟）；杀开关仅供特殊后端兜底。见 `src/agent/tools/read.rs` |
| `LAEW_WEB_OVERLAY` | `off`/`0`/`false`/`no` | 关闭**可视化蒙层与人工操作拦截**（legacy 开关，off 系 ≡ 缺省档 open；新代码用 `LAEW_WEB_GUARD`）。headed 页面半透明蒙层 + CDP `Input.setIgnoreInputEvents` 输入拦截，人工可看不可点；Agent 输入类动作自动「先解后锁」，截图自动避让蒙层，`request_human` 默认 `unlock_page=true` 提问期间解锁、收口复锁。仅 headed 生效。见 `src/agent/browser_overlay.rs` |
| `LAEW_WEB_GUARD` | `locked`（缺省）/ `open` | **页面管控缺省档位**：`locked` 屏蔽（人工可看不可点）/ `open` 非屏蔽（人工可直接操作）/ `partial` 部分屏蔽（**环境变量不支持**，须在 `open` 参数或 `set_guard` 传 `allow_selectors`/`block_selectors`）。级联：open 显式 `guard` > legacy `overlay` > 本变量 > `LAEW_WEB_OVERLAY` > locked。运行时 `control(set_guard)`。详见「领域概念 · 页面管控三档」 |
| `LAEW_WEB_LOCKED_SCROLL` | `off`/`0`/`false`/`no` | 关闭 locked 档**「可滚动观看」DOM 输入闸**（默认开启），回退 CDP 全量输入拦截（连滚轮一起拦，人工只能看不能滚）。开启时 locked 缺省改用 DOM 选择性事件闸（window 捕获拦点击/键盘/拖拽/触摸，**只拦 `isTrusted=true`**，放行滚轮/滚动/鼠标移动）；闸注入失败 fail-closed 回退 CDP。见 `src/agent/browser_overlay.rs` |
| `LAEW_WEB_HITL_SCOPE` | `off`/`0`/`false`/`no` | 关闭**凭证区域探测**（第 167 轮起仅影响 `inspect(info=blockers)` 的 `credential_zones` 返回）。HITL 解锁语义已与其解耦：`request_human(unlock_page=true)` 缺省**整页 open 安全全面解除屏蔽**（按页面实际态判定，多窗口下各页独立；人工自由操作，点『提交 / 继续』交还控制权，收口按该页解锁前实际态恢复）；显式 `allow_selectors` 才 partial 挖洞（自动并入挑战层选择器 + 应用前覆盖验证，洞没罩住挑战回退整页 open）。见 `src/agent/tools/mcp_web_use/unlock_zone.rs`、`docs/MCP_Web_Use/08-浏览器进程保护与凭证区域部分放行.md` |
| `LAEW_WEB_BLOCKER_PROBE` | `off`/`0`/`false`/`no` | 关闭**自动阻断感知**（默认开启）。输入类动作(20 个)/`wait`/导航类/**eval_js** 完成后自动探测人工验证挑战——图片语义题等**题面无「验证码」关键词**的风控弹窗靠弹层结构(高 z-index 模态层 + 九宫格小图/CSS 背景图网格/验证码尺寸 iframe/canvas/验证文案)打分识别，命中附 `data.blocker_alert` + `next_action=request_human` + `human_assist`；`inspect(blockers)` = 关键词（**纯 ASCII 走整词边界**）∪ 弹层语义双通道；**输入前置硬闸 6002** 只认结构证据（防常驻文案永久锁死输入），`params.force=true` 逃生口；子类型只有真能填码的才叫 `captcha`，其余 `manual_verify`。详见「领域概念 · 自动阻断感知」。见 `src/agent/tools/mcp_web_use/blocker_probe.rs` |
| `LAEW_HITL_AUTO` | `off`/`0`/`false`/`no` | 关闭**人工介入自动弹出（auto-HITL）**（默认开启）。开启时输入硬闸 6002 与收尾探针**首次**命中直接弹人工介入弹窗等人工处理（不再只回提示载荷等 LLM 再调 `request_human`，省一轮往返、根治「无视 6002 提示继续输入」），`data.auto_hitl{trigger,status,human_response?}` 回报应答；6002 信封码恒不变（动作未执行不伪造成功）。守卫：非 sequence/batch 上下文（批内单步 150s 预算会截断人工等待，批内回退提示载荷）+ hub 无 pending（不排队叠加）。见 `src/agent/tools/mcp_web_use/control/hitl_ctl.rs` |
| `LAEW_WEB_EVAL_TIMEOUT_MS` | 正整数（默认 `30000`） | eval_js 执行超时兜底：实测风控弹窗拖住主线程后 `Runtime.evaluate` 永不返回，裸 await 把整个 `sequence` 卡死 56s 直到用户 Ctrl-C。`eval_js_string` 全量走该超时（`act_eval_js` 按 `await_promise` 分档 false=本变量/true=60s，显式 `params.timeout_ms` 优先、上限 120s）。超时返 2002 并附处置指引。见 `src/agent/tools/mcp_web_use/mod.rs` |
| `LAEW_WEB_MAX_WINDOWS` | 正整数（默认 `4`） | **多窗口并行**（双站同时工作）单浏览器进程允许同时存在的 Agent 窗口数上限；按去重 CDP windowId 计（同窗口多 Tab 不占额度），超限 `open(new_window=true)` 返 1001 提示先关闲置页。0/非法回退默认。见 `docs/MCP_Web_Use/22-多窗口并行双站同时工作.md` |
| `LAEW_WEB_MAX_TABS` | 正整数（默认 `8`，下限 `2`） | **标签页总量上限**（2026-10-14 闲鱼实测，与上一条互补）。`adopt_spawned_pages` 收编新页前按本上限丢弃超额**新候选**（不关既有工作页），响应附 `dropped_pages`+`dropped_hint`。背景：`LAEW_WEB_MAX_WINDOWS` 按 windowId 计额，同窗口多 Tab 不占额度，实测同一商品页被重复打开单任务残留 12 个 tab 一个上限都没触发。同轮另做**同 URL 去重**（`browser_noise::page_url_key`，键相同才判同页，`?page=1` 与 `?page=2` 不去重）与 `spawned_page_ids` 全量回报 |
| `LAEW_WEB_GEO` | `grant`（默认）/ `deny` / `prompt` | **Chrome 原生地理位置授权弹窗自动处置**（第 162 轮）。原生权限气泡是浏览器 UI 层，blocker_probe/HITL 均不可见，页面 JS 挂起等授权会把 Agent 工具链拖住（实测闲鱼任务人工点完后「等了好长时间才继续」）。`grant`：open/navigate 导航完成后对页面 origin CDP `Browser.setPermission` 预授权 + 启动参数 `--deny-permission-prompts` 兜底拒掉其它原生权限弹窗，弹窗零出现零等待；`deny`：预置 denied（页面自行降级，如退回手动选地区）；`prompt`：完全不干预（原生行为，调试）。connect 模式（接管用户已登录 Chrome）不干预。见 `src/agent/browser_perm.rs` |
| `LAEW_WEB_BACKGROUND_GATE` | `off`/`0`/`false`/`no` | 关闭**多标签页后台挂起治理**（默认开启）。Chrome 一个窗口只有一个激活标签页，其余转后台挂起（渲染暂停/rAF 停摆/懒加载不触发/定时器节流）：①`control(screenshot)`/`inspect(screenshot|ocr)` 在后台页执行前返 **6003**（`params.focus=true` 自动前台化后执行/`force=true` 强制）；②其余动作放行但响应附 `page_background` 提示；③`list` 逐页 `foreground` 即时探测标记（不缓存，人工切页实时反映）+ 页数 ≥6 附 `tabs_hint`；④`close(page_id="background")` 批量关后台页（探测失败跳过；无前台页时拒绝）；⑤`control(bring_to_front)` 标签页前台化。见 `src/agent/tools/mcp_web_use/page_activity.rs`、`docs/MCP_Web_Use/18` |
| `LAEW_WEB_FOCUS` | `keep`（默认）/ `always` / `never` | **焦点租约**（`src/agent/browser_focus.rs`，`docs/MCP_Web_Use/21`）：Chrome 置前/抢焦的「借-还」治理。`keep`：置前走「抬升不抢焦」（macOS AXRaise / Windows SetWindowPos NOACTIVATE），且 open / `control(bring_to_front)` / 6003 `focus=true` / `request_human` 四处自动抢焦点点位在动作收口后把键盘焦点**归还给调用前的前台应用**（响应附 `focus_restored` 对账；request_human 提问结束才归还）；`always`：旧行为（`Page.bringToFront` 置前激活、不归还，全程盯屏观看用）；`never`：完全不碰焦点（不抬升不归还）。CDP 操作本不需要键盘焦点，根治「Chrome 抢焦后用户在其它窗口打字灌进 Chrome」。快照/归还走 osascript/PowerShell 系统工具子进程（零新 crate），Linux no-op fail-open |
| `LAEW_WEB_PACE` | `off`/`0`/`false`/`no` | 关闭**站点导航节奏治理**（默认开启，`src/agent/browser_pace.rs`，`docs/MCP_Web_Use/23`）。两条防线：①同一注册域两次显式导航（open/navigate/new_window）间隔不足 `LAEW_WEB_PACE_MIN_INTERVAL_MS`（默认 2500）时透明补齐等待；②任何探针（推送式 blocker_probe / 拉取式 `inspect(blockers)`）命中**频控/限流横幅**（「由于访问频繁导致无法搜索」「请求频繁」等，kind=`rate_limited`）即标记该域冷却 `LAEW_WEB_RATE_COOLDOWN_SECS`（默认 240s，幂等取更晚），冷却期内同域导航返 **6004**（附 host/remaining_secs/advice/force_hint，动作不执行；剩余 ≤ `LAEW_WEB_PACE_AUTO_WAIT_SECS` 默认 45s 时透明等待后放行）；`params.force=true` 逃生口。背景：京东 1W 组装机实测 76 分钟任务，~117 次 search 导航触发频控后横幅不在阻断模式表（旧表只有「访问过于频繁」），Agent 继续硬闯 8 分钟直到人工终止。域按注册域归一（search/www/list.jd.com → jd.com），IP 字面量原样 |
| `LAEW_WEB_WIPE_COOLDOWN_SECS` | 正整数（默认 `90`） | **整页抹除(site_wipe)冷却时长**（第三条防线，2026-10-14 猎聘实测，`browser_pace.rs`）。显式导航落地 `about:blank` 记域级连击（落地非 blank 清零），连续 2 次触发 `cause=site_wipe` 冷却，同域导航返 **6004**（`data.cause=site_wipe`，专属建议=换全新 profile/`reuse_existing`，非「冷却等待」）；`close(page_id="all")` 回收浏览器时冷却与连击清零（全新 profile=全新会话身份）。短于频控 240s：抹除是会话/指纹级打标，等时间无意义，冷却只为强制换策略。`params.force=true` 逃生口；`LAEW_WEB_PACE=off` 一并关闭 |
| `LAEW_WEB_WIPE_TOTAL_THRESHOLD` | 正整数（默认 `3`） | **整页抹除总账阈值**（2026-10-14 第二轮，猎聘 Chrome 拉起风暴根治，`browser_pace.rs`）。`wipe_total` 是跨浏览器实例的累计抹除总账：`clear_all`/`close(page_id="all")` **不清**，落地非 blank 才清零。同域累计 ≥ 阈值后，对 **laew 身份**导航（launch 全新实例 / auto_relaunch 调试实例，`guard_navigation` 第三参）**持久返 6004**（`data.persistent=true`）——指纹级打标站点对 CDP 拉起环境本身打标，换全新 profile 照样被抹，`close(all)` 清连击逃逸的漏洞被堵死；用户日常 Chrome 接管（真实指纹）不熔断。`params.force=true` 逃生口 |
| `LAEW_WEB_RISK_COOLDOWN_SECS` | 正整数（默认 `300`） | **风控挑战冷却时长**（第四条防线，第 172 轮 2026-10-14 天猫 baxia 滑块 punish 实测）。探针/inspect 命中 baxia 厂商容器（`.baxia-dialog`/`iframe[src*="punish"]`）即标记同注册域 `cause=risk_challenge` 冷却，同域导航返 **6004**（专属建议=别重试加剧风控/人工验证/换身份）；`request_human` 收口人工已完成验证（Answered）自动解除——人工解了滑块=风控已解除。nc/geetest/yidun 容器只用于挑战识别（slider 子类 + 专属 advice）不触发冷却；`close(page_id="all")` 清零。`params.force=true` 逃生口；`LAEW_WEB_PACE=off` 一并关闭。**2026-10-14 闲鱼实测两处语义修正**：①打标**取更早**不续期（挑战是状态不是事件，复验探针反复命中同一挑战不该续命——旧「取更晚」+ 复验重打标使 `remaining_secs` 三次恒 299、冷却永无解除窗口）；②`clear_risk_challenge` 后开 **90s 抑制窗**（`RISK_SUPPRESS_SECS`），窗内同域不再打标（人工解完 → Agent 按指引复验 blockers，复验不得反手锁死）。另：`login_like` 的 baxia 容器是**登录弹层**不是 punish，不触发冷却。见 `src/agent/tools/mcp_web_use/blocker_probe.rs`、`src/agent/browser_pace.rs` |

| `LAEW_SUBAGENT_MAX_DEPTH` | 0..=3 | 动态子 Agent 嵌套层数上限，默认 `1`（子 Agent 为叶子，不能再启动）。`0` = 完全禁止（等价关闭）。 |
| `LAEW_SUBAGENT_MAX_PARALLEL` | 1..=8 | 会话级并发槽位，默认 `3`（对齐 atomcode `Semaphore(3)`）。 |
| `LAEW_SUBAGENT_MAX_TOTAL` | 1..=64 | 会话级累计启动预算，默认 `8`（防 token 失控；耗尽返回信封 `2001`）。 |
| `LAEW_SUBAGENT_MAX_ITERATIONS` | 4..=32 | 单个子 Agent 迭代上限，默认 `12`。 |
| `LAEW_SUBAGENT_TIMEOUT_SECS` | 10..=3600 | 单个子 Agent 墙钟超时，默认 `300`（超时记 `status=timeout`）。 |
| `LAEW_SUBAGENT_PERSIST` | `off`/`0`/`false`/`no` | 关闭动态子 Agent **运行记录持久化**（默认开启）。关闭后不写不读 SQLite `subagent_run` 表，`SubAgent(action="history")` 返回 `code=0 + persist:false + 空列表`（工具面与提示词不变；`LAEW_SELF_SPAWN=off` 时本就零写入）。见 `src/config/subagent_run.rs` |
| `LAEW_SUBAGENT_RUN_KEEP` | 0..=20000 | 启动期保留的最新运行记录条数，默认 `500`（`0` = 不清理）。TUI bootstrap 与 `-p`/`-f` 单轮模式启动时各执行一次 trim（fail-open）。 |
| `LAEW_CHROME_DEBUG_PORT` | 正整数（默认 `9222`） | 复用已登录浏览器的 CDP 调试端口探测覆盖：`open(reuse_existing=true)` 依次尝试 `localhost`/`[::1]`/`127.0.0.1` 该端口（Chrome 154+ 的 DevTools HTTP 端点只服务 IPv6 loopback 连接）；非法值回退默认。见 `src/agent/browser_reuse.rs`、`docs/MCP_Web_Use/06-复用已登录浏览器会话.md` |
| `LAEW_WIN_THEME` | `off`/`0`/`false`/`no` | 关闭 **Windows 弹窗 macOS 风格主题**（默认开启）。开启时 HITL 弹窗及其触控板子窗统一走 `src/agent/human_ui/windows_theme.rs`（观感唯一真源）：浅/深双色板随系统注册表 `AppsUseLightTheme` 自动切换，自绘圆角主/次按钮 + macOS 开关（NSSwitch 形态多选项）+ 卡片式说明区/输入框，DWM 标题栏与客户区同色 + Win11 圆角；`theme_all_process_windows` 每秒 `EnumWindows` 自遍历本进程全部顶层窗口补挂 chrome（日后新窗口自动被覆盖，控件级主题仍须按规则接入）。`light`/`dark` 强制档位。**新增 Windows 弹窗一律复用该模块，禁止在窗口代码硬编码颜色**（见 `docs/MCP_Web_Use/27`） |

## 领域概念（改代码前必读）

- **程序目录（原「根目录」）** = `laew` 二进制所在目录（`current_exe()` 父目录）。**只**用于定位程序自带资源（如 `__hitl-dialog` 子进程可执行文件），**不再是任何产物的落点**；启动横幅对应行标签为「程序目录:」。
- **工作目录** = 启动命令时所在目录。Bash/Read/Write 工具的相对路径基准。两者可能不同，勿混淆。
- **产物目录** = `LAEW_ARTIFACT_DIR` > 启动时工作目录 > 程序目录（`src/artifact_root.rs::artifact_root`，进程级单点捕获）。**所有运行期产物一律落这里**：`plans/`（Plan 方案）、`DebugReport/`（-debug 报告）、`AuditTrail/`（决策审计 JSONL）、`BashSpill/`（Bash 大输出）、`logs/laew-tui.log`、`CrashReport/`、`EvalSpill/`（eval_js 大结果）、`WebShots/`（截图/OCR/HITL 附图/data: 下载）。`safe_join` 保证 `..`、前导 `/`、Windows 盘符分量无法逃出产物根。**例外：SQLite `LsmAgentEmergentWork.db` 仍在程序目录**（接入点与会话记忆是跨工作目录的全局状态，迁移会丢存量；仅当程序目录不可写时回退 `<产物根>/.laew/`）。启动横幅有独立「产物目录:」行。
- **当前项目说明文件** = 以**工作目录**为基准按五级链发现：非空 `CLAUDE.md` → 非空 `AGENTS.md` → 非空 `README.md` →（都没有但根目录层有其它 `*.md` 时，程序化分析后**自动生成 `README.md`** 落盘使用）→ 空（不注入）。Yolo 在每个 Session **首次处理**时，把「工作目录路径 + 说明文件内容」包装成带 `<<<LAEW:PROJECT_CONTEXT>>>` 标记的独立 user 消息插入上下文 index 0（标记探测幂等、与用户提示词严格隔离），设计见 `docs/Yolo项目上下文注入/`。TUI 横幅的「项目说明:」行为纯探测展示。
- **接入记录（完整的大模型接入记录）** = `protocol(anthropic|openai) + provider_name + model_name + end_point + api_key` 五元组 + `context_max_size`(上下文最大 Token 数,默认 800K,`0` = 不限制/关闭自动压缩；支持 `800000`/`800K`/`1M` 写法)，存 SQLite `providers` 表，可多条，`is_active` 唯一。存量库打开时自动迁移补列回填默认值。
- **接入点补全**：Anthropic → `{end_point}/v1/messages`；OpenAI → `{end_point}/chat/completions`；尾部 `/` 自动裁剪。
- **运行日志文件（输出 log 文件）**：`--debug` / `--info`（含 `-debug` / `-info` 单横线与 `--DEBUG` 等大小写变体）在工作目录生成 `llaew_YYYYMMDD_HHMMSS.log`（时间戳 = laew 启动时刻，精确到秒，本地时区；已 gitignore）。`--debug` → DEBUG 级（含每轮 LLM 请求元信息/响应思考文本与工具意图全文），`--info` → INFO 级主干事件；实现复用 tracing 双层订阅器（原控制台层行为不变 + 文件层 `src/logging.rs`），埋点覆盖全部 8 角色的感知（任务输入/Agent 会话）/ 决策（Yolo 分类/Plan/Main-Work 拆解/QC 报告）/ 执行（WorkFlow 单元/Context 压缩/SessionContext 摘要/任务收口）/ 思考（LLM 响应文本与 tool_calls）与全部工具调用（名称/参数/结果/耗时，`agent_loop.rs` 中央埋点）。TUI 横幅追加「启动时间」行（常显）与「日志文件」行（仅 `--debug`/`--info` 时显示相对化路径 + 级别，`/clear` `/new` 重印横幅仍可见；启动时刻由 main 单点捕获，横幅显示与日志文件名时间戳严格同刻，`TuiLaunch` 传递）。
- **工具定义协议差异**：Anthropic 用 `tools[].{name,description,input_schema}`；OpenAI 用 `tools[].{type:"function",function:{name,description,parameters}}`（function 风格）。
- **任务锚点（TargetAnchor，`safety/target_anchor.rs`）**：从用户原文机械抽取的目标硬约束（**不经 LLM**，不可幻觉、不可丢失），全局单例（与 `BrowserManager::global` 同构）。跨五层设防：L0 输入保真 + `prompt_lines/prompt_chars` 提交留痕；L1 编排器澄清门（Yolo 填 `target_status="unresolved"`，或机械通道命中「指代 + 零主机」→ `OrchestrationOutcome::DirectAnswer` 回问用户，不进 WorkFlow）；L2 Main-Work 提示词约束 + `plan_validate.rs` 澄清单元阻断（伪门在 DAG 里无法暂停等用户，拆了就被秒级打回）；L3 `MCP_Web_Use action=open/navigate/new_tab` 显式 URL 越界返 `code=6001`（新开 6xxx「范围约束」段，避开 5xxx Use_MCP 与 4xxx HITL；信封附 `workaround` 指路——锚点站点的集团同域组成部分（如 tmall.com 的搜索落地 s.taobao.com）改走**页面内表单提交/点击页面内链接**由页面自身发起导航，不经 open/navigate/new_tab；2026-10-14 天猫实测不指路模型要白烧 1~2 轮自己悟），存活页面提示按 anchor 二分为「✓ 可复用 / ⚠ 禁止复用」；L4 QC 目标一致性硬门（用户指定 X、实际操作 Y 一律 Fail 且 `retryable=false`）+ `target_drift` 强信号（命中即 `is_failed`）。**`detect_target_drift` 只扫显式导航位 URL**（`open` 顶层 `url`；`control` 里 `navigate`/`new_tab`/`download` 的 `params.url`；`sequence`/`batch` 的 `steps[].params.url`），跳过 `ok=false`，且裸主机通道拒识「标识符续接」片段（`div.kr-loading-more-button` 曾被截成主机 `div.kr` 误报强信号，把 QC 的 pass 降级为 Fail，合法单元不可恢复）。开关 `LAEW_TARGET_ANCHOR=off` 全关；`LAEW_TARGET_ANCHOR_BLOCK=off` 仅关 L3 阻断保留观察。
- **网页取证纪律**：网页任务的证据只能来自 `MCP_Web_Use`——`MCP_Web_Use(action=open)` 返 `2001`/`3001` 时**如实报告不可达并停止**，禁止改用 `curl/ping/nc` 等 Bash 网络命令探测任务目标站点（实测事故：28 次 Bash 对 8 次 MCP_Web_Use，Bash 抓的内容 QC 无法与浏览器轨迹对账，与伪造同质）。三层设防：(1) 提示词 `src/agent/system_prompt/web_evidence.rs` 第 20 条（仅注入持 MCP_Web_Use 的 SubAgent-Work，并修正了 Main-Work「任务前提验证」原本鼓励 `curl` 验证 URL 的三处表述）；(2) 运行时硬闸门 `src/agent/safety/web_evidence.rs`（五条件全中才拦：开关未关 + 是 Bash + 工具面含 MCP_Web_Use + 命令命中网络取证程序 + 命令里的 host 落在任务锚点域内；host 抽取含**裸 IP 字面量**通道，`nc -z 10.0.0.9 8080` 这类无 scheme 的内网诊断才拦得住）；(3) `LAEW_WEB_EVIDENCE=off` 全关回退。设计见 `docs/MCP_Web_Use/04-网页取证纪律.md`。
- **人工介入可见性**：`request_human` 的「该走人工」不再只写在提示词里，OCR 报错/空文本（`control(screenshot,ocr=true)` 与 `inspect(info=ocr)`）与 `inspect(info=blockers)` 命中阻断时，响应附 `next_action="request_human"` + 完整 `human_assist` 载荷（ready/reason/message/options/可复制的 call/反伪造 rule）；输入硬闸 6002 与收尾探针首次命中则**直接自动弹出**（auto-HITL，`data.auto_hitl` 回报应答，`LAEW_HITL_AUTO=off` 回退提示模式，见「领域概念 · 自动阻断感知」⑦）；`4001 Unavailable` 附 `human_ui_diagnostics{gui_enabled,platform,reason_hint,env_switch}` 供排障「为什么没弹窗」；`-p`/`-f` 单轮模式新增 250ms 轮询协程排空 `AssistEvent` 并打弹窗通知行（此前弹窗照弹、应答照回填，但终端零输出）。见 `docs/MCP_Web_Use/02-人工介入与窗口可视化方案.md` §5.0/§5.1.1/§5.3。
- **人工介入弹窗 = Rust 原生子进程（`human_ui/`，设计见 `docs/MCP_Web_Use/03`）**：`laew __hitl-dialog <payload.json>` 隐藏子进程（与 `__browser-watchdog` 同构）——macOS 走 AppKit FFI（手写 objc runtime 绑定 `objc_msgSend`，零新 crate，dlopen 装框架）、Windows 走 Win32（**WM_SIZE 全量重排**根治最大化）。更早的 JXA/PowerShell 脚本层反复出现「最大化后按钮无响应」，根因是布局双账本 + 回读桥接值 + 编译器不可见，已**整体删除**（含 `LAEW_HUMAN_UI_SCRIPT` / `.laew/human_ui/` 动态加载）。铁律：布局是 Rust 纯函数单一真源（`layout_frames`/`content_height` 同源参数化 + 单测锁死预算对账），最大化/还原只用自身设定值重排、**永不回读窗口 frame**；文案可选中复制靠**建控件前先装编辑菜单**（标准 responder action 沿 first responder 派发）+ `make_label` 统一 `selectable=YES`，另有「📋 复制」一键写系统剪贴板（倒计时行每秒重绘会冲掉人工的拖选，故必须有一键复制）。主按钮「继续」双语义（有输入=把补充说明交给 Agent，空输入=选项 1 的「交棒」动作显性化，人工操作完成后点它把流程与控制权交还 Agent）+「⏱ +2分钟」延长等待（stdout `extend` 中间行 → 父进程逐行流式读 → 推迟 hub 截止时刻，总上限 30 分钟，根治人工整链登录超时→QC 重试→二次弹窗）+「解除屏蔽/恢复屏蔽」互切按钮（2026-10-13，stdout `guard` 中间行 `{"status":"guard","id","unlock"}` → `hub::guard_toggle_from_ui` → `act_request_human` 安装的 handler 对关联页整页 open / 按本页解锁前快照恢复，含导航重放协程收口 abort；初始标题按 payload `guard_unlocked` 页面实际放行态，TUI 打 `GuiGuardToggled` 通知行；根治实测「登录界面，不能点击，优化一下」——解锁入口交到人工手里）。`code=0` 的 `next_hint` 按「输码 vs 已完成」分流（`hitl_hint::looks_like_code_text` 让「人工已完成操作」的整句指示不被当验证码填进页面）。**验证码相关功能已移除**（2026-10-13）：图片区（NSImageView/STM_SETIMAGE）、「查看原图/🔍 放大」、「⛶ 最大化」按钮（验证码图片放大遗物，说明区本就可滚动，Windows 保留系统标题栏最大化）、`reason=captcha` 自动截图采集链（`captcha_crop` 模块整链删除）；`request_human` 附图仅保留显式 `params.image_path` 透传（TUI 兜底打印路径）。**选项多选与自动弹出**（2026-10-13 第二轮，`control/hitl_ctl.rs`）：`request_human(params.multi_select=true)` 选项渲染为复选框组（macOS NSSwitchButton / Windows 自绘 macOS 开关，杀开关 off 回退 BS_AUTOCHECKBOX），点「继续」收集勾选，应答编码 `1. a；3. c`（`join_multi_answer`，数字前缀与 `is_option_pick_answer` 兼容归入选项应答分流，空勾选回退选项 1 交棒）；**auto-HITL 自动弹出** —— 输入硬闸 6002 与收尾探针首次命中时直接弹人工介入（不再只回提示载荷等 LLM 再调，省一轮往返），`data.auto_hitl{trigger,status,human_response?}` 对账（6002 信封码恒不变：动作未执行不伪造成功），守卫 = `LAEW_HITL_AUTO` 开 + 非 sequence 上下文（`IN_SEQUENCE` task_local，批内 150s 单步预算会截断人工等待）+ hub 无 pending；`act_request_human` 重构为 `HitlSpec → request_human_core → HitlReport`，LLM 显式与自动升级共用同一执行核。Windows：父进程以 `SW_HIDE` 启动时首次 `ShowWindow` 会被 STARTUPINFO 压制，追加 `SetWindowPos(HWND_TOPMOST,…,SWP_SHOWWINDOW)` 直接置 `WS_VISIBLE`。Ctrl-C 取消收口：Debug 报告跳过 LLM 评估（本地骨架秒级落盘，原评估阻塞 4 分钟且零反馈），stage 打印与 HITL 行读接入取消 token。**Windows 弹窗 macOS 风格主题**（2026-10-14，`human_ui/windows_theme.rs` 观感唯一真源，设计见 `docs/MCP_Web_Use/27`）：浅/深双色板随系统注册表自动切换；主/次按钮 owner-draw 圆角自绘（`BS_OWNERDRAW` → `WM_DRAWITEM`），多选项 = NSSwitch 形态开关（勾选态由 `opt_checked` 台账单一真源，收口只读台账）；说明区/输入框卡片化（`WM_CTLCOLOR*`）；DWM 标题栏与客户区同色 + 沉浸式深色 + Win11 圆角；`theme_all_process_windows` 每秒 `EnumWindows` 自遍历本进程全部顶层窗口补挂 chrome。**日后新增 Windows 弹窗一律复用该模块**（类背景刷/按钮样式/`WM_DRAWITEM`/`WM_CTLCOLOR*` 分发到主题模块，禁止窗口代码硬编码颜色）；杀开关 `LAEW_WIN_THEME=off` 回退系统原生观感。**焦点不抢夺与假应答根治**（2026-10-16，BOSS 直聘实测）：弹窗此前 `SetForegroundWindow`+`SetFocus`/`makeKeyAndOrderFront`+`activateIgnoringOtherApps` 抢占系统键盘焦点，用户在其它窗口打字时下一次 Enter 被 `IsDialogMessageW` 路由到默认按钮 → 20~48s 内产生「假人工已响应」（选项1空输入交棒），复验不过→再弹窗→再误触空转，人机协作闭环被机械绕过。修复三件套：①弹窗**置顶显示但不抢键盘焦点**（Windows `SWP_NOACTIVATE`+`FlashWindowEx` 闪任务栏 / macOS `orderFrontRegardless`，不再 activate/makeFirstResponder，删除 0.05s focusOnce 二次断言）；②主按钮去 `BS_DEFPUSHBUTTON`，Enter 不再路由提交，**提交必须鼠标点击**；③「输入框持有焦点时的 Enter」由消息循环显式转发提交（Windows WM_KEYDOWN 拦截 / macOS 输入框 action，真交互语义保留）。**孤儿自退**（同轮）：子进程全部 stdout 写点（收口行/extend/guard/relay 中间行）改 `dialog_main::emit_line_or_exit`——写失败=父进程管道已关闭，静默 `exit(0)`（窗口随进程销毁），根治父进程被终止后子进程 `println!` panic `os error 232` 生成 CrashReport；父进程侧 Ctrl-C 取消路径同步补 `HumanAssistHub::cancel_pending()`（与 Err/正常收口路径对齐）。**参数宽松解析**（同轮）：`request_human` 的 `options` 接受直接数组与 LLM 实测嵌套形状 `{"item":[…]}`（含双层），`timeout_ms` 接受数字字符串 `"300000"`（`parse_hitl_options`/`de_u64_lenient_json`，单测锁死）。**按钮白底白字修复**（2026-10-15 人工报障「按钮看不清楚都是白色」，见 `docs/MCP_Web_Use/28`）：根因 `draw_text_line` 未设背景模式——DC 默认 `OPAQUE` + 默认白底，`DrawTextW` 先把整个按钮矩形擦白再画字（主按钮蓝底白字被刷成白字白底）；修复 = 绘制前 `SetBkMode(TRANSPARENT)`（调用处均已先用填充图元铺满控件面）+ `draw_switch_row` 行底自绘（此前行底依赖 OPAQUE 擦除副作用）。
- **注入命名空间隐身 + 风控页识别 + 侵入式物理输入中继（第 168 轮，天猫/京东双站实测）**：① **隐身**——向页面注入的 guard/highlight/follow/探测脚本全部改用进程级随机命名空间（`src/agent/browser_ns.rs`，如 `__lzn8f3a2b1`；模板写 `%NS%` 占位符、`render_ns`/`ns_global`/`agent_attr` 取真实名；`data-laew-agent` → `data-<ns>agent`；`eval_js_string` 网关集中替换），脚本尾部把全部 API 入口 `Object.defineProperty` 改**不可枚举**（`for…in`/`Object.keys` 不可见）——根治风控 JS 一行枚举即得「laew」固定签名（实测京东人工扫码登录后返回 `.jd-main-risk-fallback` 风控页）；launch 后首启一次**隐身自检**（`navigator.webdriver` 必须 false、window 可枚举键不得残留 `laew`，违例 WARN）。② **风控页识别**——`blocker_probe` 关键词组（当前页面异常/请刷新或切换账户/内容太火爆了/访问过于频繁→`manual_verify`）+ `page_health` 新增 `reason=risk_page`（空 DOM 判定前先匹配，附「自动 reload 无效」处置指引）。③ **侵入式/非侵入式**——人工介入弹窗按钮行新增「🎮 侵入式」互切（「非侵入式」入口 = 既有「🔓 解除屏蔽」按钮本身,文案保持原文案不变——实测改名会造成「文案跑到按钮位置上」的错位观感）：侵入式时弹窗切换为**触控板**（macOS `LAEWRelayPad: NSView` 动态子类 / Windows `LAEWHITLRELAYPAD` 独立窗口类，`pad_frame` 纯函数从布局派生），人工在触控板拖动/按键 → stdout `relay` 中间行 → `hub::relay_from_ui` → `BrowserManager::relay_mouse/relay_key` 以 CDP `Input.dispatchMouseEvent/dispatchKeyEvent` 回放——`isTrusted=true` 真物理输入、坐标级派发穿透跨域 iframe、不依赖浏览器焦点（根治实测天猫登录后滑块「请按住滑块拖到最右边」人工无论如何操作不了：跨域 iframe + 页面 JS 防护 + 窗口焦点语义三层叠加）；映射区 = 显式 `request_human(params.relay_rect)` > `relay_rect_probe` 挑战层自动探测 > 全视口；拖拽 move 按 ~10px 插值补点（滑块轨迹校验）；session_start 放行本页管控/置前窗口，session_end/收口恢复快照。方案 `tmpPlan/20261013_1625_登录风控页与滑块弹窗兼容方案.md`。
- **TUI 多行粘贴保真（`tui/paste.rs` + `tui/input/mod.rs`）**：粘贴瞬间回显前 4 行预览，提交时按展开版逐行完整回显（marker 化，不把换行压成空格）。**Enter 提交前的三级粘贴判定**：40ms 队列探测命中 → 粘贴换行；未命中但**击键节奏武装**（尾部 300ms 窗口内 ≥8 字符到达，≈27 字/秒，人类连击 ≈10~12 字/秒不误伤）→ 延长观望 300ms 捕捉 ConPTY 分块滞后；均无 → 正常提交（零额外延迟）。探测命中后走**长窗口尾部积累**（600ms 静默 × 10s × 64KB 上限），触发 Enter 前的 buffer 行作首行前缀与尾部全文拼成完整整段——Windows conhost / 部分 ConPTY 不支持 bracketed paste，多行提示词会逐行拆成多次提交（实测 9 行只活下来 2 行，含目标 URL 的行整体丢失 → Agent 改派桌面路线），这是该机制的根因。macOS 正常，纯因终端 bracketed paste 生效。方案 `tmpPlan/2026-10-11_Windows平台测试三缺陷根治方案.md`。
- **崩溃形态备忘**：`chat_loop` 回复检测曾用旧 OCR 快照的字节长度切新快照，两次 OCR 无前缀关系，字节下标落进 CJK 多字节字符中间即 panic（`start byte index 28 is not a char boundary`）；已改 `ocr_snapshot_grew` 纯字符级公共前缀差分，单测锁死。
- **终端守卫**：`src/tui/term_guard.rs`。macOS 实测事故——终端窗口/标签被关后
  pty master 关闭，slave 的 `read` 恒返回 EOF，而 crossterm 0.27 的 `event::poll(timeout)`
  与 `event::read` 在该状态下**都会永久 hang**（`mio.rs:95-120` 内层循环不检查超时、
  `Ok(0)` 不 break），表现为 100% CPU 空转；又因 `shutdown.rs` 为 SIGHUP/SIGTERM 注册的
  handler 只写标志位不触发退出（注册 handler 还会抑制内核默认终止动作），主线程卡在
  `read` 里回不到 `tui/mod.rs` 的检查点，导致**进程永不退出且 `kill` 无效**（只有 `kill -9`
  能杀）。修复三层：(1) 独立 `std::thread` 看门狗轮询 fd 0 的 `POLLHUP`（实测 master 关闭后
  立即触发、只订阅 `POLLHUP` 即够），命中即清理退出；shutdown 已触发超 10s 宽限也强制退出，
  让 `kill` 真正有效。(2) 收口**不能**用 `shutdown::terminal_restore_sync`——它抢 Rust stdio
  全局互斥锁，主线程卡死时永不释放，守卫线程会一起卡死（日志已打「强制退出」但进程仍活着），
  改用 `libc::write(2)` 直写 fd 1/2 的 `force_terminal_restore`，并加 3s 无条件退出的保险丝
  线程。(3) 全部 4 个读事件入口（`read_line_inner` 主循环 / 快速输入排空 / `drain_paste_burst` /
  `engine.rs::read_key` 即 `/provider *` 子屏）补「进 `event::read` 前先做 0 超时挂断探测」。
  `isatty(0)` 门控，非 TTY（管道 / CI / e2e）一律跳过，零误杀（实测健康 TUI 连续 45s 不被杀）。
  同轮顺带修：两条提前 `return Err` 路径跳过 `terminal_restore`（关掉后终端滚动区/底部面板残留）、
  TUI 退出不调 `BrowserManager::shutdown`（Windows 无 atexit 又无 Drop，Chrome 留后台）、
  `macos_legacy` 的 `open.spawn` 泄漏 zombie、`human_ui` payload 临时文件改 RAII 清理。
  详见 `docs/TUI输入处理常见陷阱与修复记录.md` §N 与
  `tmpPlan/2026-10-09_终端消失后进程空转不退根治.md`。
- **浏览器可见模式与窗口几何（唯一真源 `src/agent/browser_mode.rs` + `browser_viewport.rs`）**：缺省 = 可见 `headed`（此前 `run_open` 硬编码 `Hidden` 且 `from_env_or_default` 零调用，"弹不弹窗"全看 LLM 有没有自己传 `mode`）；无 GUI 会话（`CI`/`SSH_*`/Linux 无 `DISPLAY`·`WAYLAND_DISPLAY`/macOS `launchctl managername` 非 `Aqua`）自动回退 `hidden`，`LAEW_BROWSER_MODE` 真正接线。**窗口尺寸规则（默认行为，第 158 轮 2026-10-12 起不铺满）**：headed 导航完成后**保持当前窗口尺寸**（`fill=true` 取 `cur` 而非 `cap`）——首次打开保持 `--window-size` 的 1920×1080，**用户/人工手动调整过窗口大小则以其为准**（实测用户反馈「打开新页面总是全屏覆盖手动调整的窗口大小」后改）；仍保留「超屏收边到 `cap`」与「视口下限 720p 抬升」保障，`positioned`（left/top）仅在超屏收边时下发，抬升保持原位置。随后 `Page.bringToFront` 置前，响应回 `data.headed_window{fill, positioned, applied, measured}`。**页面视口随窗口自适应**（下限 1280×720）：窗口定后视口即窗口内容区，内容仍超视口再走 2K 扩展。**视口铺满根治（第 161 轮，内网站点实测「白屏 + 显示区域没有铺满 Chrome」）**：chromiumoxide `BrowserConfig` 默认 `viewport=Some(800×600)`，每个 page target 初始化都发 `Emulation.setDeviceMetricsOverride(800,600)` 把页面视口钉死（`hide()` 老式 headless 会自动清默认视口，但本项目三档用的 `with_head()`/`new_headless_mode()` 都不清）——窗口外框 1920×1080 内容只渲染左上 800×600、其余全白；第 156 轮「macOS inner 恒 800×600 是布局时序」实为该覆盖钉死（Windows 同样中招），`chrome_height_raw=480` 失真同源。修复：launch 时 headed `viewport(None)`（禁用视口仿真，Playwright `viewport:null` 同款，页面视口=窗口内容区 ≈1920×940，实测 1920×993）；hidden/new_headless 显式 `viewport=窗口尺寸`（恒 1920×1080）；connect 模式不受影响（`Browser::connect` 走 `HandlerConfig::default()`，viewport 本就 None）。铁律：**顺序** —— `fit_headed_window` 必须**早于** `fit_viewport_to_content`，因为 `setWindowBounds` 会清除 `Emulation.setDeviceMetricsOverride`，晚一步会把刚设的视口抹掉；显式传 `window_width/window_height` 同样只收边不铺满。connect 模式整路早退（不调尺寸不置前）。chrome 高度走 `plausible_chrome_height`（区间 `[40,300]` 外 + "inner 恰为 800×600 且 outer 不等"双判据——第 161 轮后 inner 已是真值，该判据退化为防御性兜底）回退常量 140，响应同时回报 `chrome_height_raw`/`chrome_height_fallback` 供对账。**多标签页断流根治（第 158 轮）**：点击链接/`window.open` 派生新 tab 时，新 tab 初始停在 `chrome://new-tab-page`（标题非空曾逃过噪声判定被误收编为 `spawned_page_id`，后续操作全打到空白新标签页 = Agent「失去控制」）；现 `is_noise_page` 对 new-tab-page 系列不看标题直接判噪声 + adopt 等待条件放宽（只要有噪声页就等 500ms 复查导航结果）+ 多候选 http/https 优先排序。**首启空标签页清扫 + 空白页诊断 + 句柄自愈（第 159 轮，内网站点实测）**：① launch 后 Chrome 自带的初始空页签被 `BrowserManager::new_page` 收尾清扫（首启且 launch 模式，`is_noise_url` **不看标题**判定——初始页签早期 URL 可能还是 about:blank 而标题已非空；connect 模式不碰用户页签），不再「先空页签后目标页签」；② 启动参数全模式 `disable_default_args` + 自选集：**剔除 `--enable-automation`**（chromiumoxide DEFAULT_ARGS 携带、`navigator.webdriver=true` 会被 WAF 反爬直接返回空 HTML）+ `--disable-blink-features=AutomationControlled` + `--ignore-certificate-errors`（自签名证书/自定义域名直达）+ `--disable-features` 禁 LocalNetworkAccess/PrivateNetworkAccess 系列（Chrome 154「公网域名→私网 IP」子资源静默拦截）；③ **Network 域先于 goto 开启** + 事件缓冲新增 `Network.loadingFailed` 采集（`error_text`/`blocked_reason`），空白页时主文档状态码（实测内网站点 WAF 对登录页返回 HTTP 400 空 HTML——与证书/域名无关）与拦截证据直接可见，不再盲猜；④ 修复两处恒 SyntaxError 的注入 JS：`wait(selector)` 模板 `async()=>{…}()`（须整体包裹 `(async()=>{…})()`）与 `inspect(dom)` 对象字面量多余分号；⑤ `ensure_page_alive`/`heal_page`：人工地址栏回车导航等导致 CDP 会话掉线时按 `target_id` 自愈重建句柄（重挂监听/管控/跟随器），工具层 `ensure_page` 全量走自愈。设计见 `docs/MCP_Web_Use/16`。**主文档健康诊断 page_health（第 161 轮）**：open（新建 + 同 URL/origin 复用两条路径）导航完成后当场聚合——主文档状态（扫 Network 环形缓冲 `phase=response` 且 `url==final_url` 条目）+ 空文档判定（排除 `[data-<ns>agent]` 注入层后 DOM 元素数 ≤3（第 168 轮起属性名随进程级随机命名空间,见 browser_ns.rs），**纯结构证据**，不叠加 body 文本门槛——注入层文案恒计入 innerText 会把真空页误判为非空）；命中即附 `data.page_health{blank, reason=main_doc_error|empty_dom, main_doc_status, note, next_action}`（不命中不带字段零噪声），`main_doc_error`（状态 ≥400）直接建议改道 `reuse_existing`/`request_human`，`empty_dom`（2xx/未知但 DOM 空）软提示先 wait 复查——根治「WAF 400 空白页 Agent 盲试 reload 5 轮 3 分钟」（内网站点实测）；WAF 拒绝本身是服务器侧反爬决策，客户端定位是首轮可见的结构化信号而非根治。设计见 `docs/MCP_Web_Use/17`。**多页面协作模型**：一个 Agent 可控制多个页面（`sequence`/`batch` 经 `current_page`/`last_spawned` 跨页流动），一个页面也可专属一个 SubAgent（page_id 随机 hex 互不冲突）；Agent 间数据传递经文件系统落盘 + `session_memory` 摘要链 + SubAgent 结果汇总。设计见 `docs/MCP_Web_Use/01-设计与解决方案.md` §19/§24、`14-窗口铺满置前与弃权如实转达.md`。
- **页面管控三档（guard，唯一最新版 `docs/MCP_Web_Use/13`，实现 `browser_overlay.rs`）**：与浏览器可见性、人工介入三维正交。`locked` 屏蔽（缺省，人工可看不可点）→ `open` 非屏蔽（人工可直接操作，用于人工登录先行流）→ `partial` 部分屏蔽（DOM 盾区：黑名单红盾罩 / 白名单垂直条带分解挖洞）。工具面 `open(guard?/allow_selectors?/block_selectors?/guard_note?)` + `control(set_guard)`，legacy `overlay`/`set_overlay` 兼容。locked 缺省用 **DOM 选择性输入闸**（window 捕获拦 `isTrusted=true` 的点击/键盘/拖拽，页面自合成事件放行保持 Agent eval_js 能力面等价，**滚轮/滚动放行** —— 人工可自由上下翻阅观看），闸注入失败 fail-closed 回退 CDP `Input.setIgnoreInputEvents`；**实测铁律：CDP 输入拦截同时拦掉 CDP 合成输入**，因此输入类动作一律"先让路后复原"（`GuardLiftKind`：`IgnoreLift` 解 CDP / `GateSuspend` 挂闸 / `VisualSuspend` 隐盾）。Agent 自主选档注入系统提示词 §17/§20。杀开关 `LAEW_WEB_OVERLAY=off`、`LAEW_WEB_LOCKED_SCROLL=off`。
- **HITL 凭证区域部分放行与浏览器进程保护（`docs/MCP_Web_Use/08`）**：① **绝不关闭用户已打开的 Chrome** —— 代码库不存在任何按进程名杀浏览器的路径（`auto_relaunch` 改为"复制登录态 → 独立 `--user-data-dir` 另启实例并存 → 接管"），laew 只对自己 launch 的 `laew_browser_*` profile 有生杀权；② `request_human(unlock_page=true)` 提问期间**安全全面解除屏蔽**（第 167 轮，用户钦定方向）：缺省整页 open——按**页面实际态**（`__laewGuardMode`，非全局配置——多窗口并行下 A 页 set_guard(open) 曾把 B 页 HITL 解锁条件短路，京东登录页人工被锁死）判定需要解锁的页面切 open，状态条「🔓 人工介入中·页面已全面解锁，可自由操作；完成后回弹窗点『提交 / 继续』」，人工自由拖滑块/扫码/登录，点『提交 / 继续』交还控制权；收口按**该页解锁前实际态快照**恢复（不再拿全局配置灌页面）；提问期间主 frame 导航自动重放放行态。背景：第 144 轮自动探测 partial 的白名单是 `:nth-of-type` 位置型选择器，React SPA 上漂移后洞消失，天猫滑块被盾罩盖死（实测双站比价，tmpPlan/20261013_134500）。显式 `allow_selectors` 才走 partial 挖洞：自动并入 `blocker_probe` 的挑战层选择器（类名/id 优先，稳定）+ 应用前覆盖验证（挑战层或其内部 iframe/canvas 被洞并集覆盖 ≥80% 才算数），洞没罩住挑战 fail-open 回退整页 open；盾区白名单在**子 frame 零命中时不再整 frame 加盾**（否则跨域验证码 iframe 内部会被从里面锁死）；`inspect(info=blockers)` 命中时附 `data.credential_zones` 选择器。杀开关 `LAEW_WEB_HITL_SCOPE=off`。
- **页面自刷新治理 / 登录墙 / 指纹体检（第 172 轮，`src/agent/page_liveness.rs` + `mcp_web_use/login_wall.rs` + `src/agent/fingerprint.rs`）**：站点对「一次性全新 profile + 自动化浏览器」会做自杀式反爬 —— **原地 reload 或整页抹除**（2026-10-14 BOSS 直聘实测：静置 30s 零动作后 `performance.timeOrigin` 变化、页面全局标记丢失、URL/标题不变；同参数无 CDP 的纯 Chrome 同样被抹，排除我方工具链与 CDP 附着）。四类通用信号：① **登录墙** `data.login_wall`（URL 登录语义 token `/login`·`/web/user`·`/passport`·`/authorize`·`redirect=` 或 DOM 密码框/登录按钮且无会话标记）—— 服务端 302 到登录页时透出，Agent 直接 `request_human(reason=login)`，禁止反复 navigate 试探（实测旧实现只回请求 URL，模型连烧 3 轮才发现「要登录」，还把首页推荐位卡片当职位列表抓）；② **落地透明** navigate/back/forward/reload 恒附 `requested_url`/`final_url`/`redirected`；③ **页面不稳定** `data.page_liveness`（90s 窗口内**站点自发**重载 ≥3 次或 30s 内 ≥2 次；主动/自发严格分离，单次重载不告警；每页 5s 节流采样）—— **`request_human` 发出前采样一次，不稳定时把 ⚠️ 提示前置到弹窗正文首行**（含「换稳定页面 / open(reuse_existing=true) / 不要反复刷新」），根治「人工在弹窗里输入不了」；④ **指纹体检** `data.fingerprint`。另：连接级失败（`oneshot canceled` 等）归一为可行动文案并就地清句柄（`new_page_impl` 入口先活探，死了走 `purge_browser`），**原样重试同一条 `open` 一次通常即可恢复**（实测接管已死调试实例会污染单例，导致连冷启动都全灭）。设计见 `docs/MCP_Web_Use/25-页面自刷新治理与登录墙信号.md`。**明确不做**：验证码打码平台、住宅代理池轮换、协议逆向直调后端接口 —— 被拦时唯一合规改道是接管用户已登录浏览器或人工登录。
- **反检测档位（非侵入式 / 侵入式，第 174 轮；第 175 轮自洽性强化，`src/agent/stealth.rs` + `browser_ns.rs` + `browser.rs` + `browser_viewport.rs`，`docs/MCP_Web_Use/26` + `29`）**：CDP 附着在页面侧留下可读痕迹（`navigator.webdriver` / WebGL 兜底串 / 空 plugins·languages / 无头档 UA 里的 `HeadlessChrome` / **我们自己改写原生函数造成的破绽** / **屏幕几何不自洽**），站点风控据此加权评分。三档：`off` 零处置 / `fidelity` **非侵入式**（缺省，只移除自动化泄漏 + 回填页面自报真值，真实值一律原样放行）/ `hardened` **侵入式**（保真之上再让「改过」不可反查，`Function.prototype.toString` 原生化）。注入脚本由三块按档组合（`CORE_JS` webdriver 覆写 → `FIDELITY_JS` 保真回填 → `HARDENED_JS` 原生化），`stealth_js_for(mode)` 唯一真源；页面创建/复用/自愈/派生四条路径共用 `inject_stealth(page, inner.stealth)`。**三条铁律**：① 被改写属性/函数的**描述符形态必须与原生一致**（WebIDL 操作 `enumerable: true`、`Function.prototype.toString` `enumerable: false`），否则压制手法本身成为签名；② 同一文档**同档重复注入必须零动作**（脚本头 `window.<ns>StealthRev` 幂等闸），否则包装器叠加、原生化失效；③（第 175 轮新增）**回填值必须与原生对象形态同构，而不只是「有值」—— 形状错误的回填比不回填更危险**（缺插件只是弱信号，错形状的 PluginArray 是强信号），故 `FIDELITY_JS` ② 用「实例→私有中间原型→真原型」三层链（`Object.create(PluginArray.prototype)` 等）而非对象字面量，`mimeTypes` 按 type **去重为 2 条**，元素字段挂中间原型 getter；**绝不凭空补本机没有的字段**（真机实测全新 profile 的真实 Chrome `Object.keys(chrome)` 恒为 `[app,csi,loadTimes]`、**没有 `chrome.runtime`**，补一个空 runtime 既违反铁律又可被一行识破）。**屏幕几何自洽（第 175 轮）**：chromiumoxide 0.9 的视口覆盖只发 width/height、**不发 `screen_width`/`screen_height`**（源码已核）→ 页面读到 `screen=800x600`（无头默认值）而 `outer=1920`，`outer > screen` 是**物理不可能**（真实显示器宽度永远 ≥ 窗口宽度），属零成本风控硬判据；故两档 headless 与 2K 扩展档（`fit_viewport_to_content`）的 `SetDeviceMetricsOverride` 均补 screen 字段（取值即本次覆盖尺寸，非编造分辨率），headed / connect 不受影响（不覆盖视口本就自洽）。`data.stealth{mode,env_default,fidelity,hardened,kill_switch}` 随导航/只读动作收尾回报；`data.fingerprint` 自证码增至六条（`headless_ua` / `webgl_descriptor_tampered` / `patched_functions_visible` / `screen_geometry_impossible` / `screen_headless_default` / `plugin_shape_mismatch`）。边界：**低熵 WebGL VENDOR/RENDERER（7936/7937）不得改写** —— 真实 Chrome 恒报 `WebKit`/`WebKit WebGL`，改写反而制造不一致；**不改验证码与风控判定逻辑本身**；connect 模式（接管用户浏览器）零注入契约不变；不做 Canvas/字体伪造、行为仿真、代理池、打码、协议逆向（§三）。**验证方式**：Node 从 `browser_ns.rs` 抽取三块 JS 按 `%NS%` 规则渲染，注入真实 Chrome 155 跑「零注入/fidelity/hardened」三态差分与「伪造对象 vs 原生对象」逐字段对账（仓库外脚本，见 `docs/MCP_Web_Use/29` §七）。**第 176 轮：回填形态自洽强化**（`docs/MCP_Web_Use/30`）—— 回填动作的**形态细节本身即签名**，四处缺口闭合：① Notification/permissions 一致性**全量覆盖**三种 `Notification.permission` 值并用**精确映射**（`default↔prompt` 语义一致、`granted↔granted`/`denied↔denied`）判定不一致，根治「简单 `!==` 把语义一致的 default vs prompt 误判为不一致并改写、反而制造不一致」+「只在 denied 时触发、granted 不一致不处理」；② `navigator.languages` 回填形态（**第 178 轮方向修正**：真机四档实测含裸环境 `Object.isFrozen(navigator.languages) === true` —— 规范里类型是 `FrozenArray<DOMString>`，**原生就是冻结的**，第 176 轮据「原生非 frozen」去掉 freeze 恰好搞反，回填现已恢复 `Object.freeze` 且自证项反相为 `languages_not_frozen`）；③ plugins/mimeTypes 的 `Symbol.iterator` 返回的迭代器**补自反方法**（`iterator[Symbol.iterator]` 是函数，ECMAScript 规范要求）；④ fingerprint 自证项扩展 `languages_frozen`/`iterator_self_iterable` 两条告警码，让上述形态破绽在 `data.fingerprint` 运行时可见（而非等风控打标）。**第 177 轮：注入时序 + 跨档自洽 + 漏检自证**（`docs/MCP_Web_Use/31`）—— 前三轮的判据都在**空间维度**（字段值/对象形态像不像原生），本轮补三个新维度：① **时序**——`inject_stealth` 从 `goto` 之后移到之前（此前 `run_immediately` 只补救当前文档，首个文档的页面脚本已在 document-start 读到未处置的 `navigator.webdriver=true` 与空 plugins 并已上报风控信标，同页第二次导航起才正常，易误诊为站点随机风控）；② **跨轮**——幂等闸改块位掩码 + 原生化前 `__adoptAll()` 重新认领 + `__rehardenIterator` 重装迭代器（详见上方 `LAEW_WEB_STEALTH` 行）；另补两处**从未登记**的改写（permissions 一致性的 `get: () => want` 未登记箭头、迭代器 `next` 裸函数，原生化只登记了外层生成器）与 **`refresh` 只属 `PluginArray` 不属 `MimeTypeArray`**（真机实测：`MimeTypeArray.prototype` 自有属性恒为 `length/item/namedItem/constructor`，无 `refresh`；旧实现对两个数组体一律挂，多出一个原生没有的键）；**`Plugin.mimeTypes` 一度被补上又已回退** —— 真机 `'mimeTypes' in navigator.plugins[0]` 为 `false`，WebIDL 的 `Plugin` 接口压根没这个属性，Node 垫片「验证通过」是拿自己虚构的 shim 对账自己虚构的字段（自证）；**形态判据一律以真机读数为准，不以 mock 垫片为准**，原生基线已钉成冒烟断言 `stealth_native_shape_baseline_smoke`；③ **漏检面**——`data.fingerprint` 新增 `automation_globals`（`cdc_*`/`domAutomation`/`__selenium*`/`__playwright*` 等前缀扫描；我们经 CDP 驱动不产生这些键，命中即硬判据 `automation_global_leak`）、`patched_fn_leak`（逐入口问 `toString`，点名回报漏登记处）、`focus_visibility`（`visible` 却 `hasFocus()=false`）、`injected_ns_keys`（注入层 ns 键数，**只观测不告警**）；顶层新增 **`risk_tier`**（`ok`/`warn`/`high`，`fingerprint.rs::HARD_CODES` 收敛）把十几条 suspicious 压成一个可执行信号：**high 就别在拉起环境继续调参，直接 `open(reuse_existing=true)` 接管真实浏览器**。验证除 Rust 单测外，把渲染脚本导出后**在 Node 里配最小 DOM 垫片真跑**（保真形态逐字段对账 / 侵入式全入口`[native code]` / 同档重复注入零动作 / 跨档升级重新认领）；**时序改动必须有真机验证**——`stealth_document_start_timing_smoke` 让 loopback 页面在 `<head>` 内联脚本（**解析期**取样，事后 eval 取不到时序）记录 document-start 读数，实测修复后首个文档即 `webdriver=false`（修复前该断言失败）；`stealth_native_shape_baseline_smoke` 把四条原生形态基线钉成断言防再犯臆测。见 `31` §七。**第 178 轮：判据真机复核 + 首文档盲区根治**（`docs/MCP_Web_Use/32`）—— 前四轮判据全建立在「推断」上（规范推理 / Node 垫片 / mock），本轮拿到真机上逐条对账（四档配置：headed/hidden × 缺省档/`stealth=off`），结果**推翻三条自证**并量化一处时序盲区：① `languages_frozen` 方向反了（见上）；② **`focus_visibility_mismatch` 退出硬判据**（真机有头档恒 `has_focus=false` —— 缺省 `LAEW_WEB_FOCUS=keep` 主动归还焦点是我们自己的设计；无头档反而 `true`，与理论相反；且真实用户窗口被遮挡时同样是 visible+无焦点，属**普通浏览常态**）→ 字段降为只观测；③ **`injected_ns_keys` 正则修真**（旧 `^__lz[0-9a-f]{8}` 撞不上真实形态 `__lzn`+8hex，该自证自诞生起恒报 0）→ 真机实测**有头档 30 个 / 无头档 1 个 / 裸环境 0 个**，新增 `injected_ns_symbols`（原生基线 0）为下一轮「30 个字符串键收敛成 1 个 symbol 键」提供验收基线；④ 回填分支同批对齐 `navigator.pdfViewerEnabled`（避免「5 个 PDF 插件 + 查看器不可用」的自造矛盾）+ 自证 `pdf_viewer_plugin_mismatch`；⑤ 新增**服务端侧取证**（`tests/stealth_reveal_smoke.rs`：本地服务器记录 `User-Agent`/`Sec-CH-UA`/请求序号）+ **回填替身真机差分**（强制触发回填后与同机原生逐字段对账，本轮靠它查出四处中间原型缺 own `constructor` 并修好）。见 `32` §二/§三/§七。**第 179 轮：注入面收敛与自证升级**（`docs/MCP_Web_Use/33`）—— 第 178 轮测到的「有头档每页 30 个 `__lzn*` 字符串键」是本项目最独有的一处页面级签名（`webdriver`/WebGL 兜底串别人也有）。本轮把窗口面 API 全部挂到**一个** `window[Symbol.for(<ns>)]` 命名空间对象上（各注入脚本闭包内 `const NS = …`），`getOwnPropertyNames(window)` 字符串键 **30→0**、`getOwnPropertySymbols(window)` **0→1**；DOM 面（element id/class/`data-*`）换用**与窗口符号无关的第二随机 token**，否则「枚举 DOM id 前缀 + 读符号描述」仍能串成同一签名；命名空间内部键统一不可枚举。渲染期唯一收口 `browser_ns::render_ns` 三步：`%NS%`→窗口 token、`window.<ns><Suffix>`→`NS.<Suffix>`（后缀白名单按长度降序，防 `GuardSuspend` 抢 `GuardSuspended`）、`<ns><DomSuffix>`→`<dom_ns><DomSuffix>`；Rust 侧 eval 统一 `ns_member_expr`。自证新增 `injected_ns_string_keys`（字符串键非零=渲染链路倒退）、`ns_symbol_present`、`outer_equals_inner`（软提示）；真机冒烟 `stealth_selfcheck_smoke` 新增 5 项注入面对账。档位语义与 connect 零注入契约不变。
- **自动阻断感知与输入硬闸（`blocker_probe.rs`）**：风控弹层的题面**常常没有"验证码"关键词**（豆包实测"选出在公园能看到的事物"图片语义题），所以不能只做关键词匹配。①推送式探针：输入类动作/`wait`/导航类/**eval_js** 收尾各探测一次，取最高 z-index 模态层按结构打分（九宫格小图 48~240px、CSS 背景图网格、验证码尺寸 iframe、canvas、验证文案强/弱词、广告负信号），命中附 `data.blocker_alert` + `next_action=request_human` + `human_assist` 载荷；②`inspect(blockers)` 双通道 = 全文关键词（**纯 ASCII 关键词走整词边界**，根治页面 JSON 里 `2Fa` 子串误命中 two_factor 的连续假告警）∪ 弹层语义挑战；③**输入前置硬闸 6002**：输入类动作与可变更 eval_js 执行前只认弹层**结构证据**（不认关键词，防常驻文案永久锁死输入）命中即拦截不执行，`params.force=true` 逃生口；④子类型区分：只有真能填码的形态才叫 `captcha`，其余一律 `manual_verify`「人工核验」；⑤同指纹告警去重（重复命中只回 `repeat` 事实，不再重复 `human_assist`）；⑥HITL 提问未结束时输入类动作挂起（4003，`hitl_guard.rs`），否则收尾"先解后锁"会把人工正在拖的拼图重新锁死。sequence/batch 任一步命中 blocker 立即中断整批并顶层透出。⑦**自动弹出（2026-10-13，`hitl_ctl::auto_hitl`）**：硬闸 6002 与收尾探针首次命中且守卫放行（`LAEW_HITL_AUTO` 开 + 非 sequence 上下文 + hub 无 pending）时直接弹人工介入并等待，`data.auto_hitl` 回报结果，与 LLM 显式 `request_human` 共用 `request_human_core` 全套待遇。⑧**告警处置建议（第 169 轮）**：关键词通道命中携带 `BLOCKER_PATTERNS` 模式表专属 `advice` 做 `hint`（风控墙=「不要 reload/重复提交，人工点页面『刷新/切换账号』或换 reuse_existing」），不再一律套通用「拖拽/滑块」错位指引。杀开关 `LAEW_WEB_BLOCKER_PROBE=off`（探测）/`LAEW_HITL_AUTO=off`（自动弹出）。设计见 `docs/MCP_Web_Use/09`、`10`、`11`、`03`§4.7。⑨**厂商验证容器指纹 + 风控挑战域级冷却（第 172 轮，天猫 baxia 滑块实测 `llaew_20261014_122337.log`）**：挑战本体在跨域 iframe 内时弹层结构信号系统性残缺（实测 score=4、`slider_like=false`、`iframe_like=false` → Unknown）——探针新增全文档厂商容器扫描（`baxia`/`nc`/`geetest`/`yidun` 选择器表，`vendors` 字段），命中即升级/合成挑战条目（`challenge_type=slider` + 「按住滑块拖到最右/🎮 侵入式触控板」专属 advice）；baxia punish 命中双通道标记同域 `cause=risk_challenge` 冷却（默认 300s，`LAEW_WEB_RISK_COOLDOWN_SECS`），6004 冷却拦截按成因三分流（频控/抹除/风控挑战，laew 身份持久熔断另档），`request_human` 收口 Answered 自动解除（人工解滑块=风控解除）；nc/geetest/yidun 只识别不冷却（登录页正常组件不误伤）。根治「挑战后立刻重试搜索 → 风控升级为会话级连首页都打标」。实现 `src/agent/tools/mcp_web_use/blocker_probe.rs`、`src/agent/browser_pace.rs`。⑩**厂商容器 login 语义分流（2026-10-14 闲鱼实测）**：阿里 passport **登录组件本身挂 baxia SDK 类名**，厂商选择器命中的是「短信登录/密码登录/扫码登录」弹层而非滑块挑战——旧逻辑无条件按 `vendor_challenge_type("baxia")` 合成 `challenge_type=slider` + 「按住滑块一路拖到最右」，人工被指引去找根本不存在的滑块（截图 `WebShots/laew_web_1791987814609.png`）。现：厂商扫描时对每个命中容器判 **login 语义**（`input[type=password|tel]`/手机号密码输入框/110px+ 见方二维码图/「短信登录·密码登录·扫码登录·立即登录」文案/「登录」按钮），`login_like` 时**整体跳过挑战合成**、交给关键词通道的 `login` 条目领跑（关键词通道零产出时兜底合成登录墙条目），且**不触发风控冷却**（`punish_vendor_hit` 认 `login_like`）。滑块真出现（轨道/拖拽手柄/canvas）仍按滑块给指引
- **站点频控节奏治理与登录墙误报降噪（`browser_pace.rs`，第 171 轮，`docs/MCP_Web_Use/23`）**：京东 1W 组装机实测（76 分钟，`llaew_20261014_105403.log`）两个失守点。①**频控横幅识别**——阻断模式表新增 kind=`rate_limited`（关键词「访问频繁/频繁访问/操作频繁/请求频繁/请求过于频繁」；与既有「访问过于频繁」「操作过于频繁」互不为子串，风控墙语义不变），命中即标记同域冷却（推送探针与 `inspect(blockers)` 双通道同源幂等）；频控**不是人工可解的挑战**——`probe_alert` 专门分支不走 auto-HITL/human_assist，`next_action=cooldown` + 剩余秒数，`inspect(blockers)` 的 `suggested_action` 同步降为 `cooldown`。②**导航节奏闸 6004**——`browser_pace` 全局单例：同注册域显式导航（open/navigate/new_window/connect 复用）最小间隔透明补齐（默认 2.5s）；冷却期内同域导航返 **6004**（附 host/remaining_secs/advice/force_hint），剩余 ≤45s 透明等待放行，`params.force=true` 逃生口；域归一到注册域（search/www/list.jd.com → jd.com，IP 原样，com.cn/co.uk 等双段后缀保留三段）。③**登录墙误报降噪**——频控降级壳页头渲染登出样式「你好，请登录」而登录态实际在（实测 6 次）：`kind=login` 关键词命中**不再中断 sequence**（`step_has_blocker_alert` 排除 login；验证码/人工核验/频控仍中断），probe 载荷 `next_action` 降为 `verify_login_state`（先 inspect 复核真实登录态再决定 request_human），不再附 human_assist。杀开关 `LAEW_WEB_PACE=off`。
- **整页抹除(site_wipe)治理（`browser_pace.rs` + `page_health_field`，2026-10-14，猎聘实测 `llaew_20261014_125139.log`）**：部分站点对 CDP 会话做**会话/指纹级打标**，被标会话每次页面**完整加载后**（主文档 200 + 子资源齐全）数秒内被站点脚本 `location→about:blank` 整页抹除（人工登录同样被抹，实测「页面是黑的」「一闪而过」；换全新 profile 立即恢复——`laew_browser_*` 本就一次性随机 user-data-dir）。与既有三形态（`main_doc_error` 4xx / `risk_page` / `rate_limited` 横幅）均不同，此前 25+ 次导航硬闯空转。三件套：①**连击熔断**——`note_navigation_outcome(requested, landed)` 记域级「落地 about:blank」连击（落地非 blank 清零，第 170 轮瞬态首击自愈后即清零不误伤），连续 2 次触发 `cause=site_wipe` 冷却（默认 90s），6004 按 **cause 分流**文案（频控=「冷却后低频重试」，抹除=「换全新 profile / reuse_existing 接管日常浏览器 / request_human」，错位建议会把 LLM 引回无效等待）；`close(page_id="all")` 回收浏览器 → `WebPace::clear_all()` 清冷却与连击（恢复路径不被自己的熔断卡住）；connect 模式不记账（用户浏览器行为不误伤）。②**page_health 新增 `site_wipe` 证据段**——`page_health_field(page_id, final_url, requested_url)`：落地 about:blank 时按**请求 URL** 扫网络缓冲（此前按 final_url 匹配恒 `main_doc_status=null` 的诊断盲区），「主文档有响应 或 同域子资源 ≥5」即「曾真实加载后被抹」，reason=`site_wipe` + 证据 + 处置指引（`wipe_evidence_from_events` 纯函数单测）。③**空白页清扫**——`BrowserManager::close_blank_pages()` 在 open 新建路径前关掉注册表内 URL 为 about:blank 的死页（证据已透出无需保留；实测单任务残留 6 个空白窗口占 `LAEW_WEB_MAX_WINDOWS` 额度），响应附 `closed_blank_pages`；connect 模式零动作。同轮顺带：sequence/batch 步内 `{"action":"open","params":{…}}` 参数形态兼容（`merge_open_params`，此前 1001「缺少 url」整批 5ms 全灭）+ 未解析 `$` 前缀占位符不作 page_id（LLM 编造 `$NEW` 污染后续步骤）；TodoWrite `id` 数字字符串宽松化（schema `["integer","string"]` + `de_opt_u32_lenient`）。方案 `tmpPlan/猎聘整页抹除反爬与空白页堆积治理_20261014_133000.md`。
- **整页抹除第二轮：Chrome 拉起风暴根治与持久熔断（2026-10-14，`llaew_20261014_140053.log` 实测 9 分钟拉起 8 台浏览器）**：根因三层——①`close(page_id="all") → clear_all()` 把连击/冷却全清零，猎聘类**指纹级打标**站点（对 CDP 拉起环境本身打标）换全新 profile 照样被抹，LLM 按 6004 建议 close(all) 恰好逃逸熔断再拉一台新 Chrome（熔断形同虚设）；②**延迟抹除**发生在 open 返回后 4~10s，open 的 page_health 结算时页面健康，LLM 下一轮读到 about:blank 无任何结构化信号；③`auto_relaunch` 调试实例也是 laew 身份但被当用户 Chrome 对待（不记账、close 只断连残留窗口）。四件套：① **`wipe_total` 跨实例总账**——`clear_all` 不清、落地非 blank 才清零，累计 ≥ `LAEW_WEB_WIPE_TOTAL_THRESHOLD`（默认 3）后对 laew 身份导航**持久熔断**（6004 `data.persistent=true`，`guard_navigation` 第三参 `laew_identity` 区分 launch/auto_relaunch 实例与用户日常 Chrome，后者不熔断）；② **laew relaunch 实例注册表**（`browser_reuse.rs` pid+port 登记）+ `BrowserManager::connected_port`，connect 模式记账/生杀权精确区分身份；③ **延迟抹除复查** `deferred_wipe_check`——该域有抹除史时 open/navigate 收尾等 3.5s 复读落地 URL，命中即改写响应附 `page_health{reason:site_wipe}`，有史才复查零常态开销；④ **`close(page_id="all")` 回收 laew relaunch 实例**（`kill_relaunch_browser`，响应附 `relaunch_instance_killed`），根治空白调试 Chrome 残留桌面与 9222 占位。6004 site_wipe 建议文案同步修正（不再把「close(all) 换全新 profile」列为无脑首选）；系统提示词【遍历覆盖纪律】新增 22.3 整页抹除处置纪律。方案 `tmpPlan/20261014_142000_整页抹除反复启动Chrome风暴与持久熔断根治方案.md`。
- **输入链路与批量效率（`input_ctl.rs` + `mod.rs`，`docs/MCP_Web_Use/15`）**：①**input_text `Illegal invocation` 根治** —— 旧 JS「`HTMLInputElement.prototype` 短路 `|| textarea`」恒真短路，textarea（文心输入框正是）拿到 input 的 value setter 品牌校验必炸；改沿元素自身原型链 `getOwnPropertyDescriptor` 逐级查 `value` 访问器（input/textarea/select/contenteditable 全覆盖）；②**自动阶梯 + 回读校验**：`use_js` 缺省 auto —— JS 原生赋值 → 页内回读（len/match/head100）→ 未生效降级 CDP sendkeys → 再未生效降级 human_input 慢速逐字，响应带 `mode`/`applied`/`value_after`/`fallbacks`（消灭"LLM 漏传 `use_js` 默认 true 在受控组件必炸"的坑默认值）；③**键名规范化**：`key_code_tuple` 大小写不敏感 + 别名层（enter/esc/space/del/ins/pgup/方向键），`key_press` 支持 `ctrl+a`/`cmd+v` 组合写法（按最后 `+` 拆，尾部 `+`/非单字符后半不拆，防 `C++` 误拆），`press_sequence` 支持 `selector` 先聚焦（旧实现忽略 selector，按键打进失焦页面回显 ok 但输入框为空）+ 多字符文本 token 自动逐字；④**sequence/batch step 自动继承 page_id**（顶层 → 上一步 → 派生页；实测逐步漏传每步 1001 整批全灭）+ 批量结果瘦身（成功步 message 截 300/失败步 2000）；⑤**guard 档漂移自愈**：管控脚本版本化重装（`__laewGuardRev` 单调递增，高版本先调 `__laewGuardOff` 卸载旧监听器/调度/观察器/视觉再重装）+ locked 应用后档位复核 rearm；⑥eval_js 超时兜底 `LAEW_WEB_EVAL_TIMEOUT_MS`（默认 30s，`timeout_ms` 上限 120s）+ sequence 步级防护（步间 shutdown 探测 / 单步 150s 预算 / blocker 中断整批）；⑦**eval_js 输入侧整形**（2026-10-14，`eval_sanitize.rs::wrap_bare_toplevel_return`）：顶层裸 `return` 自动包裹 `(()=>{…})()`（深度 0 + 词法边界判定，字符串/注释/函数体内/`obj.return` 零误伤；含顶层裸 `await` 不碰——async 包裹会改 Promise 语义）——经典脚本环境顶层 `return` 必然 SyntaxError，天猫实测模型反复翻车每次烧 1~2 轮迭代。**二次整形**（2026-10-15，`shield_toplevel_lexical`，见 `docs/MCP_Web_Use/28`）：顶层 `const/let/class` 声明自动块级包裹 `{…}`——`Runtime.evaluate` 全局词法环境常驻，同名再声明必 `SyntaxError: Identifier 'x' has already been declared`（闲鱼实测 `const sleep` 二次声明白烧一轮迭代，模型给临时变量起名高度趋同）；块完成值语义不变，`var`/`function`/纯对象字面量零扰动。`act_eval_js` 入口同步接入（此前自拼 EvaluateParams 绕过网关），死代码「build 期 IIFE 重试」下沉为**求值期重试**（首次 evaluate 真失败才按剩余预算重试，不叠加整份 timeout）。
- **多标签页后台挂起治理（`page_activity.rs`，`docs/MCP_Web_Use/18`）**：Chrome 一个窗口只有一个激活标签页，其余转后台挂起（渲染暂停/rAF 停摆/懒加载不触发/定时器节流）；页面经 open/new_tab、点击/JS 派生、**人工手动切页**不断增多或换前台。唯一事实源 = 页面即时上报的 `document.visibilityState`（**不缓存**，人工切页后下一次探测立即反映）。①**6003 后台挂起闸**：`control(screenshot)`/`inspect(screenshot|ocr)` 在后台页执行前拦截（后台截图=最后一次前台渲染的旧帧，证据必然失真），`params.focus=true` 自动前台化后执行 / `force=true` 强制；闸门序 6002 → 4003 → 6003（人工核验与页面所有权优先）；②其余动作在后台页照常正确（DOM 读写/CDP 输入），响应附 `page_background` 提示；派生新页抢走前台后原页动作响应自动附后台标注；③`list` 逐页 `foreground` 标记 + 页数 ≥6 附 `tabs_hint` 收口提醒；④收口：`close(page_id="background")` 批量关后台页（探测失败跳过不关；**一个前台页都没有时整体拒绝**，防窗口最小化时一锅端）；⑤`control(bring_to_front)` 标签页前台化（与元素级 `focus` 区分）。与 guard 三档正交（bring_to_front 不触碰管控状态）；headless 页面对 CDP 恒 visible，闸门自然零命中。杀开关 `LAEW_WEB_BACKGROUND_GATE=off`。
- **屏蔽完整性与焦点租约（`browser_overlay.rs` + `browser_focus.rs`，`docs/MCP_Web_Use/21`）**：locked 档在既有 DOM 输入闸之上加**锁定态 CSS 层** —— `__laew_locked__` class + `cursor: not-allowed !important` / `user-select: none !important`（悬停任何组件显示不可点光标、拖选/双击选/Ctrl+A 失效），`GATE_EVTS` 增 `selectstart` 事件层双保险；CSS 锁与输入闸**共用挂起判据**（`applyLockCss()`，Agent「先解后锁」期间同步放开、收口复锁，不影响 Agent 操作）；partial 盾罩自带 `cursor: not-allowed`，白名单洞内保持原生交互；wheel/scroll/mousemove 仍不拦（可滚动观看）。焦点侧 `LAEW_WEB_FOCUS` 三档（`keep` 缺省借焦速还 / `always` 旧行为 / `never` 不碰焦点），接入点 = open / `control(bring_to_front)` / 6003 `focus=true` / `request_human`；提示词层【焦点与操作节奏】+「焦点纪律」条约束「先 explore/inspect 规划、再 sequence/batch 批量速决，把置前与人工介入次数压到最少」。
- **屏蔽页面状态栏与屏蔽页面进度栏（`browser_status.rs` + `web_activity.rs`，`docs/MCP_Web_Use/24`，第 171 轮）**：headed 页面右上角与左下角两条常驻告知栏，回答人工「谁在控制 / 正在干什么 / 进度到哪」。**屏蔽页面状态栏（右上角，接管原「LAEW Agent 控制中」徽标位）** 四行：①`🔵 LAEW Agent 控制中 · LsmAgentEmergentWork-SubAgent-Work`——**必须显示具体 Agent 名**（`AgentProfile.name`，多窗口并行时人工据此判断窗口归属）；②三档档位（`🔒 页面已锁定(仅观看·可滚动)` / `🔓 页面开放` / `🛡 部分锁定`，locked 文案随 `LAEW_WEB_LOCKED_SCROLL` 分流）；③静态注意事项；④`guard_note`（HITL 解锁时自动写入「人工介入中·页面已全面解锁…」）。**屏蔽页面进度栏（左下角，接管原静态状态条位）** 只显示动态：当前动作（`open https://…` / `control click #buy` / `sequence 5 步`）、迭代进度与耗时与进度条、任务摘要、最近 6 条事件时间线。实现要点：①**独立于 guard 脚本**——guard 有版本化重装（`%NS%GuardOff` 会拆掉自身视觉），两条栏必须跨重装存活，且 `browser_overlay.rs` 已近 1800 行上限；②活动数据来自 `web_activity.rs` 的 **`tokio::task_local` 台账**（并行 SubAgent 各自读到自己的快照，全局 `Mutex` 会互相覆盖导致 A 窗口显示 B 子 Agent 名字），埋点在 `agent_loop::run_session_inner` / 逐轮 `set_iter` / `tool_exec::exec_tool_call::set_tool`，任务摘要取 Session 首条**非注入块** user 消息；③推送在 `McpWebUseTool::execute` 前后 `broadcast_start` / `broadcast_done`（毫秒级 CDP 往返，sequence/batch 按批推），页面侧 500ms 自刷新耗时行；④档位变更唯一收口 `apply_page_guard`（覆盖 open / set_guard / HITL 解锁恢复 / 导航后 re-assert 四条路径）；⑤铁律：两条栏**必须挂 closed Shadow DOM**（light DOM 会被页面 `innerText` 采集品牌签名，京东风控实测）、`pointer-events:none`、**只在顶层 frame 渲染**（注入脚本会进每个新文档含跨域 iframe，天猫 baxia 滑块挑战 iframe 内长出的状态栏把人工核验控件罩死——2026-10-14 实测根治，REV 2）、**HITL 人工核验期间整体挂起**（`apply_hitl_unlock` 挂起 / 收口·恢复屏蔽·中继 session_end 复显，页面里不叠任何 Agent 视觉元素在挑战层上）、长文本 `text-overflow:ellipsis` + `title`、`set_highlight(false)` 联动隐藏、`guard_suspend_visuals`（截图取证）联动挂起、connect 模式零注入。原 guard 左下角静态状态条与右上角徽标文本已**整体删除**（`overlay_hint_id`/`CHIP_TEXT`/`setChip` 均移除）。**REV 3（2026-10-14 Windows 1080p 实测「显示不出来」根治）**：两条栏改**各自独立的 `position:fixed` 角落锚定 host**（`{ns}status_host__` 右上 / `{ns}progress_host__` 左下），不再共用全屏 `inset:0` 宿主——旧结构里两条栏是全屏 host 的 closed-shadow 子元素，页面对 `html`/`body` 施加 `transform`/`will-change`/`filter`（SPA 常见 GPU 加速手法）时 `position:fixed` 的包含块会被劫持到该祖先，整条栏落到视口外或错位。自适应走 host 的 `max-width:min(vw,px)` + shadow 内 `@media (max-width:1280px/820px)` 紧凑档（1080p 与 2K/4K 都在角落相对位置，不写死分辨率）；**自愈**：500ms 定时器调 `ensure()` + `MutationObserver` 监听 `documentElement` 子树，SPA 整树替换冲掉 host 即重建（旧实现只 `render` 不 `ensure`，两条栏消失后到下次注入前一直空窗）；**挂起自愈**：截图/HITL/挑战探测三条挂起链路若中途被打断（进程被杀/异常路径漏复显）会把 `suspended` 永久留在页面侧导致隐身，现超 3 分钟自动复显。`StatusProbe` 增补 `viewport{w,h,dpr}` / `geom[].in_view` / `geom[].host_position` / `computed{display,position,zIndex,opacity}` 供实测一眼分清「没渲染 / 被 display:none / 被挤出视口」。**REV 4（2026-10-16 视口 2K 扩展下进度栏隐身根治，USA.gov 实测）**：`fit_viewport_to_content` 的 `setDeviceMetricsOverride` 撑大**布局视口**（如 1440）后，`position:fixed;bottom` 的进度栏锚到布局视口底边——在可见窗口（如 948）之外整整 ~500px，人工看不到左下角进度栏而右上状态栏（锚 top）幸存；`visualViewport` 同报覆盖值（页面侧无法自察）。修复：Rust 侧 `BrowserManager.page_visible_box` 记账每页「覆盖前可见区」，`broadcast` 逐页随 payload 下发 `visible:[w,h]`，模板按它把进度栏 `bottom` / 状态栏 `right` 重锚回可见区（覆盖清除时移除记账恢复自然锚定）。
- **表单动作后台治理与同页去重（2026-10-14 闲鱼实测，`form_ctl.rs` + `page_activity.rs` + `browser.rs`）**：两个互相放大的失守点。①**渲染依赖型表单动作在后台页必然失败**——Chrome 后台 tab 的渲染/rAF/懒加载全被节流，下拉面板**根本不渲染**，`select_dropdown` 的可见性探测必 0 命中；screenshot/ocr 有 6003 闸 + `params.focus=true` 前台化，表单动作此前零前置处置，失败主消息「面板可能已收合/选项文本不一致」还把人带偏（实测 `select_dropdown("北京")` 连续 6 次 2002，Agent 烧 8 轮迭代）。现 `select_dropdown`/`select_option` 入口自动置前（`page_activity::auto_foreground_if_backgrounded`，响应回 `auto_foreground`），失败消息把「页面在后台/渲染被节流」提为**首要归因**。②**同页重复打开无人收口**——点击/自愈点击命中商品卡（`target=_blank`）每次开新 tab，`adopt_spawned_pages` 逐个收编成独立 `page_id` 而响应只回 `spawned.first()` 一个（实测同一商品页 `item?id=…` 被开 7~8 次、单任务残留 12 个 tab）。现 `adopt_spawned_pages_ex` 返回 `AdoptOutcome{adopted,deduped,dropped}`：**同 URL 去重**（`browser_noise::page_url_key` 归一化：去 fragment/尾斜杠/主机小写，query 全保留，**键相同才判同页**，`?page=1` 与 `?page=2` 是合法双开不去重）直接关闭重复页；`LAEW_WEB_MAX_TABS` 上限只丢新候选不关工作页；响应全量回 `spawned_page_ids`+`spawned_count`（>1 附收口提示）+`deduped_pages`+`dropped_pages`；收编后**焦点归还原工作页**（新 tab 抢前台把工作页挤到后台 → 叠加 ① 形成「失败→派生→后台→更失败」负反馈闭环）。③**选后确认闭环补强**（2026-10-15 闲鱼区域面板机制性阻塞根治，见 `docs/MCP_Web_Use/28`）：闲鱼「区域」= ant-popover，选中省份后须再点面板内动作型按钮「查看999+件宝贝」筛选才落到 URL——`confirm_panel_js` 词表 + 正则双通道（`/查看N件|查看全部|立即查看/`，面板状容器约束不变防误点页面级按钮）；选项探测循环第 2/3 次前**重踢触发器**（toggle 语义下首击可能把已开面板关回，实测 3 次探测空转判失败），响应补 `trigger_rekicked` 事实；失败文案补级联提示（省份→城市两级传 `cascader_text`）。
- **复用已登录浏览器（connect 模式，`browser_reuse.rs`）**：`open(reuse_existing=true)` 探测本机 CDP 调试端口（默认 9222，`LAEW_CHROME_DEBUG_PORT` 覆盖；依次试 `localhost`/`[::1]`/`127.0.0.1` —— Chrome 154+ 的 DevTools HTTP 端点只服务 IPv6 loopback）并 `Browser::connect` 接管保留登录态；探测失败返 **3002** + `data.relaunch_command`（转述用户执行或 `auto_relaunch=true` 自动重试）。Chrome 136+ 默认 profile 上调试端口被忽略，必须独立 `--user-data-dir`（故 auto_relaunch best-effort 复制 Cookies/Local Storage/Session Storage）。connect 语义：close 只断连、页面管控恒 open 且零注入、`mode` 固定 headed、响应带 `connect_mode:true`。Linux 无登录态可复用，登录走 `request_human`。设计见 `docs/MCP_Web_Use/06`。
- **弃权如实转达（`docs/MCP_Web_Use/14`）**：Main-Work 输出 `{"workflows":[],"summary":"不予编排…"}` 是**合法零单元计划**（模型主动弃权），此前被转成 `WorkflowParse` 并由兜底构造通用 WorkFlow 把已拒绝的任务又执行一遍 —— 实测白烧 **156.89s / 12.4 万 input token** 且终态谎报「✅ 成功」。新增 `AgentError::WorkflowAbstain` + `orchestrator::ExecFailure{Abstain}`，弃权改为如实转达用户并收口。判据是**结构性**的「零单元 + 非空 summary」，**不含任何措辞/关键词检测**，判断权始终在模型手里。
- **Chrome 默认 1080p 视口（跨平台保证）**：`MCP_Web_Use` 打开的 Chrome 浏览器在 **macOS / Windows / Linux** 三平台均以 **1920×1080（1080p）** 为默认窗口尺寸。`browser_viewport.rs::DEFAULT_WINDOW_*` 是唯一真源；headed 模式页面视口 = 窗口内容区（≈1920×940），hidden/new_headless 显式 `viewport=1920×1080`。人工手动调整窗口后以其为准（不铺满屏幕）；视口下限 1280×720 保障，内容超视口再走 2K 扩展。**全平台统一行为，无 OS 差异**。**2026-10-16 首开校正**：`--window-size` 在 Windows 150% 缩放 + 多显示器组合下不被 Chrome 精确执行（实测首开 outer 1922×1043，高短 37px），`fit_headed_window` 现对**每个浏览器实例首个窗口**做一次性 setWindowBounds 校正（容差 4px 吸收 Windows DIP 取整 ±2px；显式 `window_*` 参数与 `new_window` 平铺路径不受影响），「默认 1080p」从文档承诺变为硬保证。
- **屏蔽页面状态栏 / 进度栏常显与分辨率自适应**：headed 页面右上角（状态栏）与左下角（进度栏）两条常驻告知栏，**在 1080p / 2K / 4K 等任意分辨率下始终固定于角落可见**，用户或人工始终可见。实现：各栏独立 `position:fixed` 角落锚定 host（`{ns}status_host__` 右上 / `{ns}progress_host__` 左下），自适应走 `max-width:min(vw,px)` + `@media (max-width:1280px/820px)` 紧凑档（不写死分辨率）；窗口自适应：窗口 resize / SPA 整树替换后 500ms 自愈重建 + MutationObserver 监听保证不丢；挂起自愈超 3 分钟自动复显。**closed Shadow DOM + pointer-events:none + 仅顶层 frame 渲染**。详见「屏蔽页面状态栏与屏蔽页面进度栏」条目与 `docs/MCP_Web_Use/24`。


### 多 Agent 架构（8 角色）

| 角色 | 身份 | 职责 | 工具面 |
|------|------|------|--------|
| **Yolo Agent** | `LsmAgentEmergentWork-Yolo` | 入口层：每条输入做 目的→目标→意图 三步分析；任务**三档分类**（simple/medium/hard）；失败回流与用户建议；分类前按 ReAct 自主收集信息（避免 Google 类目的）；延迟强制 `submit_task_classification`（探索轮不注入 forced `tool_choice`、仅末轮强制收口）；**目标可解析性判定**：填 `target_status`（`explicit`/`resolved`/`unresolved`），目标不可解析时编排器直接回问用户（澄清门）不进 WorkFlow，根除「目不明确就猜一个站点」的乱跑行为；**任务该不该接由模型自行判断**，编排器无任何政策性拒绝门（判断不适合接时结论写 `direct_answer`，编排器原样返回用户） | `Read`/`Glob`/`Grep`/`Bash`（只读侦察）/ `MCP_Web_Use`（观察类 action）/ `SubAgent`（只读并行子 Agent） |
| **Plan Agent** | `LsmAgentEmergentWork-Plan` | 规划层：仅在 hard 任务时启用；输出 Markdown 方案到 `plans/{session_id}-{seq}.md` | `Read`/`Write` |
| **Main-Work Agent** | `LsmAgentEmergentWork-Main-Work` | 流程层：接收 medium/hard 任务，拆 WorkFlow 列表（Kahn 分层 + 同层并行）；编排循环 ReAct 化（TaskFocus → Verify+Decompose → Emit 三段式 + 任务前提验证硬性要求）；复用 LoopGuard 编排层原地打转同样止损；迭代预算 `max_iterations(8)` + `explore_budget(2)` | `Bash`/`Read`/`Glob`/`Grep`/`MCP_Web_Use`（编排前探查）/ `TodoWrite`/`SubAgent`（**不持** `Write`/`Edit`，流程层只编排不落源代码；不持 `MCP_Window_Use`，桌面窗口操控归 SubAgent-Work 专用） |
| **SubAgent-Work Agent** | `LsmAgentEmergentWork-SubAgent-Work` | 执行层最小单元，每个流程处理单元委派一个 SubAgent；提示词 ReAct 化（Thought→Action→Observation）+ 工具连续工作模式（同一响应里连续的 `parallel_safe` 工具 Read/Glob/Grep 用 `join_all` + `Semaphore(4)` 并发执行 + `tool_result` 保序回填）+ 无进展止损（`agent/loop_guard.rs` 进展键 = 工具名 + 参数稳定 JSON + 结果摘要，轮询等待类合法重复零误伤；`wait_like` 让 wait/纯 sleep/`SubAgent(result|history)` 透明跳过；`NUDGE_AT=2`/`ABORT_AT=3` 双阈值 + 宽限轮强提醒作为 user 消息延迟到 tool_result 回填完再推入避免破坏 Anthropic 400 配对）；runtime hints 角色化（`HintRole{Ui,Execute,Gather,Judge}` 由实际调用过什么工具决定） | `Bash`/`Read`/`Write`/`Edit`/`Glob`/`Grep` + 平台门控注入 `MCP_Window_Use`（仅 macOS/Windows）+ `MCP_Web_Use` + `Use_MCP`（通用 MCP）+ `TodoWrite` |
| **Quality-Check Agent** | `LsmAgentEmergentWork-Quality-Check` | 质检层：每个执行单元完成后必经 QC；QC LLM 错误与用户取消不消耗单元 retry 预算；**目标一致性硬门**：用户指定 X、实际操作 Y（含改派到无关站点）一律判 Fail + `retryable=false`，retr y 不会让错位目标变成正确目标 | 可选 `Read` |
| **SessionContext Agent** | `LsmAgentEmergentWork-SessionContext` | 会话层：每个用户任务完成后汇总并写入 `session_memory` 表；Yolo 下次处理时自动注入最近 N 条（默认 3）历史摘要 | 无工具 |
| **Debug Agent** | `LsmAgentEmergentWork-Debug` | 调试层：仅在 `-debug` 调试模式下启用；任务结束后对采集的 trace（各 Agent LLM 调用输入输出 / Yolo 分类 / QC 结论 / 耗时与 token / 错误）做评估，产出「任务评估 / 质量报告 / 问题报告(P0-P2) / 优化建议」四章节；报告写入**根目录** `DebugReport/debug_report_{YYYYMMDD}_{HHMMSS}_{随机6位}.md`（已 gitignore，不入库）。TUI 下报告生成**后台化**（LLM 评估分钟级不再冻结输入循环，完成后下次交互时提示；退出有界等待 20s；`LAEW_DEBUG_SYNC=1` 恢复同步） | 无工具 |
| **Compact Agent** | `LsmAgentEmergentWork-Compact` | 压缩层：Session 主上下文估算 token（字符/4 +10%）达到当前 Provider `context_max_size` 的 80% 时由 Orchestrator 自动触发，按超出幅度自动选三档压缩率（Light ≤80% / Medium ≈50% / Aggressive ≤20%），LLM 摘要失败降级本地硬截断；保护带项目上下文/历史摘要/已压缩标记的消息与最近 4 条消息；**溢出兜底（reactive）**：真实溢出（Provider 返回 `prompt is too long` / `context_length_exceeded` 类 400）时由 Agent 循环自动三级恢复——排水（截短超长 tool_result）→ 折叠（历史合并为压缩摘要）→ 暴露（原错误上抛），见 `agent/overflow.rs`（一处包裹、8 角色全生效，全会话恢复预算 4 次） | 无工具 |
| **Reflection Agent** | `LsmAgentEmergentWork-Reflection` | 反思层（Reflection 模式）：执行单元 QC 判 Fail 且将局部重试前，从**推理路径**角度复盘上一轮执行——三维度校验（逻辑自洽/事实准确/目标匹配）定位偏差，产出根因 + 跨 attempt 累积的 lessons + 策略调整（strategy_shift），注入 `unit_retry_hint` 驱动多轮迭代优化（Reflexion 架构范式）。**反思是顾问不是闸门**（解析/LLM 失败 fail-open）；机械通道零 LLM 兜底；条件触发 + 预算上限 + `LAEW_REFLECTION=off` 杀开关，成功路径零额外 LLM 调用。见 `docs/Reflection反思模式/01-设计与解决方案.md` | 无工具 |

#### 跨角色编排（`MultiAgentOrchestrator`）

用户输入 → 项目上下文注入 → Yolo 分类 →（澄清门）→ 简单档（SubAgent）/ 中档（Main→SubAgent）/ 高档（Plan→Main→SubAgent）→ Quality-Check → SessionContext 收口。

- **无程序级拒绝门**：**「这个任务该不该接」完全由模型自行判断**，编排器不做任何检测、拦截或改写。若 Yolo 判断某个任务不适合接，把结论与替代方向写进 `direct_answer`（`decomposition_plan` 留空、`task_level=simple`），编排器原样返回用户；若判断可以接，照常分级委派。提示词与 `submit_task_classification` 的 JSON Schema 中均**不含**任何政策性拒绝字段/条款（此前存在的 `refuses_task` 字段 + `safety/refusal.rs::detect_refusal` 机械短语检测 + 编排器拒绝终态门 A/B + Plan 拒绝短路已整体移除）。详见 `docs/YoloAgent设计/01-设计与解决方案.md` §2
- **同因熔断（`types::SameCauseBreaker`）**：重试环内失败原因归一化（首行截 120 字符，`normalize_failure_reason`）连续 2 次相同即提前终止重试，不再烧满 `max_retry_per_level`；Yolo 回流后计数复位。**这是删除程序级拒绝门后唯一的通用空转止动器** —— 模型自行拒绝任务时其 Plan 输出同样解析不出 WorkFlow，归一化后逐字相同，第 2 次即熔断收口（不再由编排器代替模型做价值判断）
- WorkFlow 执行时按 `depends_on` 自动 Kahn 分层（`main_work::topo_layers`），**同层无依赖的 SubAgent 自动并行**（tokio::spawn + Semaphore 上限 3，`OrchestratorConfig::max_parallel_workflows`），跨层严格串行、上游产物按层注入，失败语义与串行一致（fail-fast 回流 Yolo）。**depends_on 净化（`topo.rs::sanitize_depends_on`，第 160 轮）**：解析入口统一净化依赖条目——占位符（`无`/`none`/`null`/`-`…）直接丢弃、散文碎片从原文抽取全部已知 wf id（`先完成 wf-1 与 wf-2` → 两条边，长 id 优先防 `wf-1` 抢 `wf-10` 子串）、**id 形状但未声明**（`wf-9`）保留给拓扑层报精准错（真拼写错误不吞）。背景：实测 Main-Work/Plan 把空依赖写成 `[\无\,\与\]` 散文，旧策略整批拓扑失败 → 重试环空转 22 分钟零单元执行；净化后按形态分流，占位/散文不再阻断拓扑
**depends_on 缺边推断（`topo.rs::infer_depends_on`，第 163 轮）**：净化之后从**产物引用**反推补边——某单元文本消费了另一样本单元产出的同名文件（产出动词 `写入/保存到/生成/落盘/产出/输出到/导出`，消费动词 `读取/基于/根据/输入为/引用/依据/结合`，就近 24 字符窗口）即补 `生产者 → 消费者` 边；同单元既读又写同名文件视为单元内闭环不加边；**逐条加边前做 DFS 成环预检**（宁可漏边也不制造环，环会让 `topo_layers` 整批报错、计划作废）。背景：实测（`llaew_20261012_175823.log`）Main-Work 把 5 个单元的 `depends_on` 全留空，Kahn 判全部同层并发，消费方轮询等不到上游文件（实测 120s+75s ≈ 195s 纯空等）并最终 Read 失败，且产出最终交付物的 wf-5 被挤到队尾饿死
- **QC 结论一致性消解（`quality.rs::reconcile_verdict`，第 163 轮）**：输出 schema 规定 `issues` 仅失败时列出，故 `verdict=pass` + 非空 issues 属自相矛盾，解析时按 issues 消解为 `Fail`，**`retryable` 仍完全采信模型**（不引入价值判断）；消解痕迹追加进 `evidence`（不覆盖模型原文）供 AuditTrail 回溯。**文本关键词降级路径不参与消解**——该路径的 `issues` 装的是「JSON 解析失败」这类降级说明而非缺陷，Background：实测 Plan 的 QC `verdict=pass` / `retryable=false` 却有 623 字 issues 点名「depends_on 全部为空，未声明链式依赖，并行调度时可能乱序执行」，该硬缺陷原样进入执行层
- **QC 报告数组字段形状归一化（`quality.rs::repair_quality_json`，第 164 轮）**：模型常把 `notes`/`issues` 输出成「内嵌 JSON 数组的字符串」（`"[\"...\", \"...\"]"`），严格 `Vec<String>` 反序列化报 `invalid type: string, expected a sequence` → 整条 JSON 通道作废 → fail-closed 判 Fail。归一化：字符串先按 JSON 数组解析（命中展开，失败包单元素）；null 删键走缺省；`suggestion`/`evidence` 数组 join、标量转字符串。Background：实测（`llaew_20261013_084832.log` 09:51:26）终局 Main QC 自评 pass 的合格方案因此被杀，62 分钟/20 万 input token 任务以 failed 收口、后两份文档从未生成
- **执行-验证-修订闭环**：WorkFlow 单元 QC 判 `retryable=true` 时先在**单元级局部重试**（仅该单元，注入本单元 QC 结论，不连坐同层姊妹单元），`OrchestratorConfig::unit_retry_budget` 默认 2（单单元最多 3 次尝试，`0` = 关闭旧行为）；预算耗尽才升级到档位级重试（`max_retry_per_level=3`，`retray_hint` 回灌 Main-Work）→ Yolo 回流（`[PREVIOUS_FAILURE]` + `failure_signals`）→ Failed outcome。retry_hint 分层：attempt=0 用档位级 hint / attempt≥1 用本单元 QC issues+suggestion 覆盖 description hint 段（`apply_retry_hint_overlay`）。失败路径用量保全：`from_agent_error` 站点改为手工构造 QualityFailure 携带已发生 usage，终态用量与逐事件累加对齐
- **Reflection（反思）模式（`src/agent/reflection.rs`）**：执行—复盘—迭代的自我审视机制，与 QC（外部裁判判「产物对不对」）分工互补——Reflection 复盘**推理路径与工具策略**，三维度校验（逻辑自洽 / 事实准确 / 目标匹配）产出 `ReflectionReport{root_cause, dimensions, lessons, corrections, strategy_shift, sound}`。**条件触发**（只在 QC Fail 且即将局部重试时深度反思；成功路径零额外 LLM 调用）+ **机械基线**（`mechanical_lessons` 从 `ExecutionTrace` 结构化信号零 LLM 映射通用教训：工具错误率 / 早终止 / failure_signals / 连续失败 / doom_loop / 截断）+ **教训账本跨 attempt 累积**（`merge_lessons` 规范化去重，渲染进 `unit_retry_hint` 的「🔍 反思教训」段，总长上限 1200 字符）。**反思是顾问不是闸门**：解析/LLM 失败一律 fail-open 落回机械教训（与 QC 的 fail-closed 语义相反）。效率四件套：条件门 / `LAEW_REFLECTION_BUDGET` 预算 / `LAEW_REFLECTION_MODE=light` 零 LLM 档 / `LAEW_REFLECTION=off` 杀开关。决策审计 `record_reflection` 留痕。设计见 `docs/Reflection反思模式/01-设计与解决方案.md`

### 三个 MCP 风格工具（替代已删除的独立 Agent 角色）

| 工具 | 替代 | 能力 | 平台门控 | 设计文档 |
|------|------|------|----------|----------|
| **MCP_Window_Use** | 原 WindowUse Agent | 桌面窗口操控：单工具 `action` 枚举分发（`open`/`list`/`find`/`inspect`/`control`/`ocr`/`screenshot`）；**双路线**（Windows UIA Pattern 优先 + Win32 消息 + 物理鼠标键盘兜底；macOS AX 控件树 + 物理输入；Linux wmctrl/xdotool 物理层）；操作优先级链 T1 UIA Pattern → T2 Win32 消息 → T3 物理输入（返回 `route=uia/win32_msg/physical` 标注供 QC/Debug 对账）；修饰键+鼠标/中键同时操作；复合 `input_batch` 一次编排 ≤40 步；视口/OCR 自适应 | **仅 macOS/Windows** 运行时注册进 `builtin_registry`；Linux 走物理层兜底 | `docs/MCP_Window_Use/01-设计与解决方案.md` |
| **MCP_Web_Use** | 原 Chromium-WebUse Agent | 浏览器操控：单工具 `action` 枚举分发（`open`/`list`/`close`/`control`/`inspect`/`sequence`/`batch`/`explore`）；`control_action` 44 个写操作（含 `bring_to_front` 标签页前台化）；`inspect` 21 个观察维度（Elements/DOM/Console/Network/Cookie/localStorage/页面元信息/OCR/`blockers`/`extract` 结构化抽取/`page_state` SSR 注水直读/`coverage` 遍历台账/`visibility`）；单步 + 连续（`sequence` ≤24 steps，批内 `$page_id`/`$spawned_page_id` 占位自动跟随派生新页，**step 自动继承 page_id**）；视口基准 1080p + 2K 自动扩展；`eval_js` 三级超时兜底 + 大结果/data-url 落盘；CDP 下载管理；蓝色选中边框 + 「LAEW Agent 控制中」徽标（注入 UI——蓝框/徽标/状态条/盾区——挂 closed Shadow DOM，页面 JS 的 innerText/DOM 枚举读不到品牌签名；document-start 覆写 `navigator.webdriver`，第 169 轮京东风控墙「当前页面异常」根治，见 `src/agent/browser_ns.rs`）。**窗口几何**：缺省窗口 1920×1080、导航后置前、保持当前尺寸不铺满屏幕（人工调整过以其为准，超屏收边）；**页面视口铺满窗口内容区**（headed `viewport(None)` 视口=窗口内容区 ≈1920×940，hidden 显式 1920×1080；第 161 轮根治 chromiumoxide 默认 800×600 钉死），`open` 响应附 `data.page_health` 主文档健康诊断（主文档 ≥400 / 空 DOM 时透出证据与处置，详见「领域概念」）。**页面管控 guard 三档** locked/open/partial + 人工输入闸（详见上文「领域概念」）。**人工介入 HITL**（`human_assist.rs` 全局枢纽，滑块/短信/扫码登录等不可自动跳过的流程结构化提问，人工答复经 oneshot 回填；**2026-10-13 起支持选项多选 `multi_select` 与阻断自动弹出 auto-HITL**，见「领域概念 · 自动阻断感知」⑦）：呈现端双通道 —— 桌面弹窗优先（`laew __hitl-dialog` 原生子进程，macOS AppKit FFI / Windows Win32，持续置顶+倒计时+时间轴+验证码图片+「提交 / 继续」+「⏏ +2分钟」，`-p` 模式同样可弹），TUI 兜底行读（`/hitl 2t6x` 应答通道），弹窗失败自动降级；二者均不可用 fail-fast 4001/4002。**复用已登录浏览器**（`open(reuse_existing=true)` 接管本机已登录 Chrome，探测失败返 3002 + 重启命令；connect 模式 close 只断连、零注入）。**自动阻断感知**（图片语义验证弹窗靠弹层结构打分识别，`inspect(blockers)` 双通道 + 输入前置硬闸 6002）。**站点频控节奏闸**（频控横幅 kind=`rate_limited` 识别 + 同域冷却 6004 + 最小导航间隔，`login` 关键词命中不再中断 sequence，第 171 轮）。**输入链路**（`input_text` 原型链原生赋值 + 三级自动阶梯回读、键名别名与组合键、`clear_input` 同款）。实现分布见下方 `src/agent/tools/mcp_web_use/`，设计见 `docs/MCP_Web_Use/01-设计与解决方案.md` | 跨 Windows/macOS/Linux；Chrome→Edge→Chromium→Brave 自动探测 | `docs/MCP_Web_Use/01-设计与解决方案.md` |
| **Use_MCP** | 通用 MCP 协议调用 | 通用 MCP（Model Context Protocol）服务调用统一入口：**真 MCP 协议客户端**（JSON-RPC 2.0，与 `MCP_Web_Use`/`MCP_Window_Use` 的本地驱动封装刻意区分），连接外部 MCP server（stdio 子进程 / Streamable HTTP）；单工具 `action` 枚举分发（`list_servers`/`connect`/`list_tools`/`call_tool`/`list_resources`/`read_resource`/`close`）；server 接入记录在 SQLite `mcp_servers`（`laew mcp add|list|del|test` 维护，headers 经 Vault 加密），**LLM 不可新增 server**；懒连接 + 指数退避重连稳定性窗口 + `kill_on_drop` 防子进程泄漏；ContentBlock 四类投影降级永不丢弃；统一 JSON 信封 0/1001/3001/5001-5006；注册进 SubAgent-Work + Main-Work（编排探查），Yolo/Plan/QC 不持 | 跨平台（由外部 MCP server 决定能力） | `docs/Use_MCP/01-设计与解决方案.md` |

### 自感知动态子 Agent（Self-Awareness SubAgent）

模型自主委派通道：Agent Tools 新增 `SubAgent` 单工具（`action` 枚举 `launch` / `batch` / `list` / `result` / `cancel` / `history` / `resume` / `workflow`）。
**用户提示词显式要求**（「启动 SubAgent / 并行 / 分工 / 分别调研 / 多个 Agent」）或任务可分解为 2+ 独立子任务时，Agent 自动组建子 Agent 小队并汇总结果。

- **子 Agent 6 类型**（内置）：`general-purpose` / `explore` / `researcher` / `plan` / `code-reviewer` / `operator`
- **自定义类型**：`.laew/agents/*.md` + `~/.laew/agents/*.md` 两级发现（与 D2 自定义斜杠命令同构，共享 `src/frontmatter.rs`）；frontmatter 支持 `label/description/extends/tools/readonly/name`；正文 = 专属职责提示词；定义即生效（不缓存、不改代码不重编译）；内置 6 类 id（含别名）不可被遮蔽
- **工具面三重收窄**：`tools` 白名单 ∩ `extends` 默认 ∩ `SpawnPolicy` 上限 ∩ 真实注册表，被剔除项进 `dropped_tools`
- **运行持久化**：每次运行落 SQLite `subagent_run`（`insert running` → `update finish`，单一入口 `run_child`，fail-open）；`action="history"` 跨任务/跨会话查询（零 LLM 成本）；`action="resume"` 前序任务+结论作为种子上下文注入新子 Agent（消耗一次预算，不是进程级恢复）
- **工作流编排**（`action="workflow"`）：≤8 个 `steps[]` 声明 DAG（每步唯一 `id` + 自包含 `task` + `depends_on`）；Kahn 分层 + 同层并行 + 跨层串行；上游成功结论按受控截断注入下游；上游失败→直接下游 `skipped`、间接下游 `blocked`
- **治理**：默认 1 层嵌套（叶子不注册该工具 + 运行时 2002 双重防御）；会话级 `Semaphore(3)` + 预算 8 + 单子 Agent 12 轮 / 300s；父 `CancelToken` → `child_token` 级联取消；用量经会话台账计入 `/cost`
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

- **AgentProfile**：Agent 身份档案（名称 / 系统提示词 / 工具集 / `spawn_policy`），`work_profile` / `yolo_profile` / `dynamic_child` 工厂函数
- **Session**：进程内会话，拥有独立 Session ID 与对话上下文（context）；TUI 启动或 `/new` `/clear` 时生成新 Session
- **会话持久化**：TUI 每轮任务收口把 transcript **整快照重写**（单事务 DELETE+INSERT）到根目录 SQLite `chat_sessions`（索引行：title/turn_count/model_name/updated_at）+ `chat_turns`（轮次：`response` 人类版与 `context_response` 上下文回填版分列）；失败仅告警不打断对话；自动保留最近 50 个会话。恢复走 `/sessions` `/resume` / `--resume`（`-c`），重建后保持原 Session ID（`session_memory` 摘要链连续）、PROJECT_CONTEXT 幂等重注入
- **请求头**：两协议统一携带 `User-Agent: {AgentName}/{版本} {编译时间}`、`Authorization: Bearer {api_key}`、`X-Session-Id`；Anthropic 请求体 additionally 携带 `metadata.user_id`（含 `device_id/account_uuid/session_id/agent`）。**User-Agent 按"发起请求的 Agent 角色"逐请求注入**（`Agent::run_session_inner` 从 profile 写入 `RequestMeta.user_agent`，8 角色各自携带自身名称，抓包层面可辨识）
- **系统提示词编写原则（9 角色统一）**：所有角色提示词按两类约束切分，**改提示词前先判断某条属于哪类**——
  | 类别 | 定义 | 处置 |
  |------|------|------|
  | **政策性约束** | 约束「该不该接这个任务」（按攻击/越权/违法/违反服务条款/危害他人等性质决定拒绝） | **不写进提示词，程序也不拦截** —— 判断权完全交给模型。写成关键词清单既占 token 又与模型自身判断打架，且关键词在正常工程语境里高频出现会误伤合法任务 |
  | **运行正确性约束** | 约束「怎么把任务做对」（目标保真、证据不伪造、路径保真、能力边界、澄清门、schema） | 保留。这些各有运行时闸门配套（`target_anchor` code=6001、`web_evidence` 五条件闸门、QC 对账），删提示词即回退已根治的事故 |
  **刚性三档**：硬（必须：字段类型/schema/目标保真/证据不伪造）/ 中（默认如此，例外需理由：批量优先、ReAct 三段、迭代预算）/ 软（建议、一般、通常：收集轮次上限、拆解数量对齐）。**不要把软约束写成「禁止/必须/严禁/红线」** —— 那会让模型在边界情况不敢变通。
  **两条硬规矩**：① 提示词正文不写「第 N 轮」沿革标注（对模型零价值，只增噪声与 token；沿革归 `docs/` 与 git，`prompts_have_no_round_annotations` 单测锁死）；② 入口层角色（Yolo）的分层约定写成「默认如此 + 例外需理由」而非禁令（如「Bash 主要做只读侦察，写盘命令一般留给执行层 —— 你若判断确实需要现在动手，直接做即可」）。单测：`system_prompt/mod.rs::prompts_have_no_policy_refusal_wording` / `prompts_keep_correctness_constraints` / `prompts_have_no_round_annotations`。
- **Anthropic 三段式系统提示词**：8 角色 + WorkFlow 角色的 `system` 字段从单字符串重构为三段式，对齐 Claude Code CLI 抓包范式——
  1. **billing 计费头**（无 `cache_control`）：单行 `x-anthropic-billing-header: cc_version=...; cc_entrypoint=cli; cc_is_subagent=true;`，Anthropic 内部计费/链路字段，对模型行为零影响
  2. **identity 基础身份声明**（带 `cache_control: ephemeral`）：单行 Agent 身份 + 一句话职责，8 角色 + WorkFlow 各一份
  3. **rules 核心行为规则**（带 `cache_control: ephemeral`）：base + tools_hint + protocol_tail + 准静态 workspace_hint，等价于原 `render` 单字符串内容
  缓存复用：三段全部静态/准静态 → 缓存断点逐轮命中。**runtime hints 不进 system**：逐轮变化的进度/收口/无进展提醒经 `RequestMeta.runtime_tail` 由 wire 层拼到消息流末尾尾注（`llm/anthropic.rs::append_runtime_tail` 追加到最后一条 user/tool 消息 content 尾部，不持久化 session context），否则会摧毁 rules 块 cache_control 断点之后的全部缓存前缀（实测 46/172 次 cache_read 仅 188）。4 断点 cap 约束下，billing 无 cache / identity + rules + last tool + latest user 各一份 cache，刚好命中 cap。OpenAI 协议走 `system` 单字符串 + 同一尾注注入位。实现 `src/agent/system_prompt/mod.rs::PromptSegments` + `src/llm/anthropic.rs::convert_system_blocks_split`，Agent 循环 `meta.anthropic_segments = Some(profile.system_prompt.prompt_segments)` 注入。

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
  mod.rs           会话外壳:TuiSession 结构 + 生命周期(bootstrap/banner/provider 增删查/分支快照)+ orchestrator 装配 + run/run_with_debug 入口
  dispatch.rs      输入分发与任务输出:handle_user_input / dispatch_prompt(@提及展开 + 阶段进度协程 + SIGINT 取消)/ emit_debug_report / print_* 家族
  slash.rs         斜杠命令路由:handle_slash + run_theme/run_rewind/run_undo/run_fork/run_branches/run_switch/run_export + print_custom_commands
  provider_screen.rs  /provider 子屏桥接:list/add/del 三屏接入 + run_screen_loop(通用 Screen 栈循环,非 TTY 回退 print)
  format.rs        纯函数格式化:任务结果双版本(人类版 format_task_result / LLM 回填版 for_context)/ merge_usage / waiting_line_text / clip_cols(显示列截断)/ print_help(信息盒)/ print_record
  format_brief.rs  [tool]/[laew] 行关键字段摘要(MCP_Window_Use 焦点守卫/route/verified 等)
  audit_view.rs    /audit 决策审计可视化面板
  banner.rs        启动横幅:声明式行集(BannerData)+ textfit 自适应渲染 / 工作区·连接·日志行文本纯函数
  engine.rs        CLI 渲染引擎 —— Screen trait + Frame + 全量重绘 present
  form.rs          通用 Tab 表单状态机(被 ProviderForm 屏复用)
  hitl_view.rs     人工介入(HITL)TUI 呈现视图:请求块渲染/倒计时同行右对齐/行读映射/弹窗接管通知行与事件打印
  input.rs         单行输入(主屏用):行编辑 + 行内提示 + 补全 + **自适应底部面板**(DECSTBM 滚动区;面板顶行紧跟光标,不留白,查不到光标行时回退吸底 —— )
  paste.rs         粘贴保真层(从 input.rs 拆出):PasteRegistry 登记簿 / handle_paste_text / 粘贴预览与提交完整回显(纯函数)
  completion.rs    斜杠命令补全引擎(内置 + 自定义命令动态注册)
  commands.rs      自定义斜杠命令(D2):两级目录发现/frontmatter/占位符渲染
  export.rs        会话导出(D8):transcript 记录 + Markdown/JSON 落盘
  branches.rs      对话分支存储(D3):rewind/fork/switch/clear 前自动快照(内存态上限 10)
  mention.rs       @ 文件提及解析 + 实时路径补全 + 目录钻取
  pathfmt.rs       路径格式化辅助(相对路径展示/工作目录锚定)
  textfit.rs       显示宽度唯一真源:char_width/width/clip/clip_mid/wrap/pad + InfoBox 自适应信息盒(横幅与 /help 共用)
  theme.rs         ANSI 颜色 / mask_key 脱敏 / attrs·bg·color→ANSI 转换 集中管理
  term_guard.rs    终端守卫:看门狗线程 + POLLHUP 挂断探测 + 无锁终端还原
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

  profile.rs       AgentProfile(名称 / 系统提示词 / 工具集 / spawn_policy) + work_profile/yolo_profile/dynamic_child + User-Agent
  custom_agents.rs 自定义子 Agent 类型(D115):两级发现(.laew/agents)/frontmatter->AgentDef/ResolvedAgentType/名册可见性
  self_awareness.rs 自感知层(D114):6 环境变量配置 / 6 类子 Agent 名册 / SpawnPolicy 三档 / 静态身份段渲染(工具清单由注册表生成)
  dynamic_subagent.rs 动态子 Agent 运行时(D114):会话级 Governor(Semaphore/预算/台账/作业表/事件环) + tokio task_local 作用域 + 子 Agent 组装/运行/回收 + drain_usage
  subagent.rs      SubAgent 主逻辑(D114):launch/batch/list/result/cancel/history/resume 单工具入口
  subagent_workflow.rs SubAgent 工作流编排(D116):≤8 步 DAG + Kahn 分层 + 原子预扣预算
  decision_audit.rs 决策审计(D9-8):5 决策点 → AuditTrail/*.jsonl 三段式结构化记录
  human_assist.rs  人工介入 HITL 枢纽(D100/):pending 槽位 + oneshot + kind 标签/默认文案单一事实源 + 弹窗呈现接线(via=gui/mark_gui_failed/事件环);弹窗与 TUI 均不可用才 fail-fast
  human_ui/        人工介入弹窗 UI 呈现层:mod.rs(门面/能力探测/payload/结果解析/子进程管理)+ dialog_main.rs(__hitl-dialog 子进程入口)+ macos_dialog.rs(AppKit FFI 原生弹窗:objc runtime/控制器 IMP/布局纯函数/定时器)+ windows_dialog.rs(Win32 原生弹窗:窗口类/控件/WM_SIZE 全量重排)+ windows_theme.rs(Windows 弹窗 macOS 风格主题唯一真源:浅/深配色板/owner-draw 自绘图元/WM_CTLCOLOR 配色/DWM chrome/进程窗口自遍历;新弹窗必走)+ tests.rs
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
  orchestrator/    MultiAgentOrchestrator 总编排器目录:mod.rs(结构体+入口+进度通道) / types.rs(共享类型+QualityFailure+SameCauseBreaker 同因熔断) / pipeline.rs(handle_inner+三档链路+同因熔断止损+失败路径用量保全) / workflows.rs(分层并行+run_wf_unit) / yolo_reflow.rs(Yolo 分类封装+失败回流) / usage.rs(用量累加) / tests.rs

  quality.rs       Quality-Check Agent + 单元 QC 提示词构建 + retry 预算
  debug.rs         Debug Agent(-debug 模式):trace 评估 + 四章节报告生成
  compact.rs       CompactRunner:token 估算 / 三档选档 / 自动压缩触发 / 硬截断降级 / 保护段识别
  subagent_workflow.rs SubAgent 工作流编排(同上)

  permissions/     权限管控:mod.rs / dangerous.rs / readonly.rs / sensitive.rs
  safety/          工具层防护:mod.rs / url_safety.rs(SSRF 拦截) / prompt_injection.rs(提示注入告警,advisory 不阻断) / credentials.rs(凭证静态加密 AES-GCM) / target_anchor.rs(任务锚点,跨 5 层设防的唯一事实源) / web_evidence.rs(网页取证纪律)  —— 注意:safety/ 只做**工具层与数据层**防护,不含任何"该不该接任务"的政策性拦截(该判断归模型)
  sandbox_hook/    沙箱钩子:mod.rs(单文件,接外部 sandbox)
  skills/          Skill 系统(渐进式披露):mod.rs / registry.rs / render.rs / tools.rs / skill.rs / bundled.rs / bundled/{code-review,git-commit,test-runner}.md

  system_prompt/   SystemPrompt 组合与渲染:mod.rs / yolo_prompt.rs(Yolo 提示词块,拆出) / plan_prompt.rs(Plan 提示词块+探索阶段引导,拆出) / use_mcp_hint.rs(Use_MCP 提示词段) / web_evidence.rs(网页取证纪律) / web_extract.rs(网页内容提取纪律) / skill_catalog.rs
  tools/
    mod.rs         Tool trait + ToolRegistry(有序) + builtin_registry/yolo_registry + max_parallel_tools_from
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

    mcp_window_use/  MCP_Window_Use 工具目录(仅 macOS/Windows 注册):mod.rs(门面+共享辅助) / query.rs(action=list/find+模糊打分) / open.rs(action=open+应用启动) / inspect.rs(action=inspect/control) / vision.rs(action=ocr/screenshot) / chat.rs(微信/豆包等 IM chat_send + read_text + 焦点守卫 + 回复检测字符级差分,根治 OCR 快照字节切片 panic) / explore.rs(action=explore 树预算剪枝) / sequence.rs(run_sequence assert_text/wait_for_text OCR 兜底) / input_batch.rs(复合 action ≤40 步) / tests.rs
    mcp_web_use/     MCP_Web_Use 工具目录:mod.rs(门面+八 action 分发+视口接线+guard 级联+sequence 步级防护/超时兜底/6002 硬闸+bool_param 布尔参数兼容) / control/(41+ 个写操作:mod.rs 门面分发 + 拆分子模块)input_ctl.rs(输入动作:原型链原生赋值+三级阶梯回读+键名别名/组合键/press_sequence 聚焦) / guard_ctl.rs(页面管控:先解后锁/先隐盾后复盾/先挂闸后复位+set_guard) / follow_ctl.rs(滚动/视口/自动跟随一族) / form_ctl.rs(select_option/select_dropdown/pick_element_js:自定义下拉命中三件套可见性+自愈重试+诊断+portal 全局搜索兜底) / hitl_guard.rs(HITL 期间输入挂起闸 4003) / blocker_probe.rs(推送式探针+弹层语义打分+输入前置硬闸) / inspect.rs(20 个只读观察分发 + coverage 台账 + blockers 整词边界) / extract.rs(结构化列表抽取+页面内过滤) / page_state.rs(SSR 注水 + JSON-LD 直读) / eval_sanitize.rs(返回值净化+复合值体积闸门) / unlock_zone.rs(HITL 凭证区探测+导航重放+CDP DOM pierce 跨 frame 输入框精确挖洞 @rect) / hitl_hint.rs(应答语义分流纯函数) / visit_ledger.rs(遍历访问台账:key 只折叠翻页/游标参数,保留 `?q=`/`?cat=`/`?city=` 等身份 query) / tests.rs
    use_mcp/         Use_MCP 工具目录(通用 MCP 服务调用,真 MCP 协议客户端):mod.rs(门面+七 action 分发+JSON 信封 0/1001/3001/5001-5006) / tests.rs

  window/          窗口操控平台驱动层(MCP_Window_Use 服务实现):mod.rs(模型+WindowDriver trait+工厂) / windows.rs(UIA+Win32) / windows_input.rs / windows_ocr.rs / macos_axui.rs(AX) / macos_vision_ocr.rs(Vision OCR) / control_action.rs / fallback.rs(wmctrl/xdotool) / macos_legacy/(mod.rs FFI+Driver入口 / ax_attrs.rs / cg_event.rs CGEvent 输入底座 / inspect.rs AX 控件树遍历 / act.rs / tests.rs)
  safety/web_evidence.rs  网页取证纪律:Bash 网络取证命令识别 / 命令行 host 抽取(含裸 IP 字面量)/ 任务锚点比对 / 拒绝文案
  browser.rs       浏览器 CDP 驱动层(MCP_Web_Use 服务实现):BrowserManager 单例(page_id 注册表 + 跨平台浏览器检测 + Console/Network 缓冲 + 下载事件管理 + 生命周期回收) + fit_viewport_to_content 2K 自动扩展 + fit_headed_window 有头窗口收边
  browser_mode.rs  浏览器启动模式决策唯一真源:BrowserMode 三档枚举 + DEFAULT=Headed + from_arg/from_mode_env_value/from_legacy_headless_value/resolve + has_gui_session(CI/SSH/Linux 无 DISPLAY/macOS launchctl 非 Aqua → 无 GUI 自动回退 Hidden)
  browser_noise.rs   幽灵标签页判定
  browser_follow.rs  自动跟随滚动 + 可视性诊断(:locked 档拦滚轮,只能让页面自己停在最新;should_follow 仅「本就在底部」才吸底; visibility_note 按 human_scrollable 分流,人工上翻阅读时不强制吸底)
  browser_overlay.rs 页面管控模块:PageGuardMode/PageGuardConfig(校验/env 级联)/管控 JS 模板(蒙层+盾区两件套,烘焙 {MODE}/{ALLOW}/{BLOCK};白名单=垂直条带分解求补集;第 171 轮起不再持有任何文字元素,状态条迁 browser_status.rs)/inject_agent_guard/apply_page_guard(收口推屏蔽页面状态栏)/guard_suspend_visuals(同步挂起两条栏) + legacy apply_page_overlay/mask_set_visible 委托 + action_dispatches_input 白名单 + 单测(含 #[ignore] 真浏览器三档集成验证)
  browser_status.rs 屏蔽页面状态栏/进度栏模块:AGENT_STATUS_JS_TEMPLATE(右上角状态栏 + 左下角进度栏,closed Shadow)/status_host_id/status_bar_id/progress_bar_id/guard_text(三档档位与静态注意事项)/describe_web_action(动作标签)/build_payload/ensure/push/broadcast_start/broadcast_done/set_visible/set_suspended + 单测
  web_activity.rs Agent 活动台账:tokio::task_local CURRENT(并行 SubAgent 各读自己快照)/begin/scope/current/set_iter/set_tool/task_summary_from_context(跳过注入块取首条用户原文)/clip_chars + 单测
  browser_viewport.rs 窗口与视口尺寸决策:DEFAULT_WINDOW_*(1920×1080)/VIEWPORT_FIT_MAX_*(2K)/MIN_VIEWPORT_*(1280×720 视口下限)/HEADED_SCREEN_MARGIN + default_window_size/viewport_fit_plan(只放大)/headed_window_plan(按 screen.avail* 收边 + chrome 补偿)/测量 JS + 单测
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

mcp/               通用 MCP 客户端层(Use_MCP 服务实现):mod.rs(类型+ContentBlock 投影+错误码映射) / jsonrpc.rs(JSON-RPC 2.0 帧) / transport.rs(stdio NDJSON + Streamable HTTP) / client.rs(握手/tools/list 分页折叠/tools/call/resources) / manager.rs(懒连接+指数退避稳定性窗口) / tests.rs

database/          SQLite 全栈基础设施:mod.rs / schema.rs(SCHEMA_VERSION + apply_migrations) / pragmas.rs(WAL + 6 PRAGMA) / paths.rs / models.rs / chat_store.rs(chat_sessions/chat_turns 整快照重写) / provider.rs(providers 五元组 + context_max_size) / mcp_server.rs(mcp_servers 接入记录 + headers Vault 加密)

config/            全局配置:mod.rs / agent_memory.rs(agent_memory DAO) / agent_message.rs(agent_message DAO) / session_memory.rs(session_memory DAO + SessionContext 摘要) / subagent_run.rs(动态子 Agent 运行记录 DAO:insert/finish/list/get/孤儿标记/trim)
```

统一消息模型是关键设计：Agent 循环与工具层永远不接触协议细节，协议差异封闭在 `llm/*` 两个客户端内部。

## TUI 界面（独立 CLI 渲染引擎）

### 屏幕拓扑

- **自适应底部面板**：输入面板不再恒定吸屏幕底部，而是**顶行紧跟当前光标行**（`Layout::panel_top_for` 纯函数决策，`enter_pinned` 落地）。启动横幅这类「短输出 + 大终端」场景实测原会固定留 16 行空白（观感像一串空回车），现输出与面板之间**恒不留空行**；内容接近屏底时自动吸底，行为与旧版一致。滚动区底行改为 `panel_top`（`scroll_bottom`），输出填满时游标正好停在面板正上方。**坑**：查光标行必须**在 `ESC[r`(DECRST)之前**发 `ESC[6n` —— 本机 tmux 实测 `ESC[r` 会让紧随其后的 DSR 回包变成 `1;1`（shell 复现：无 `ESC[r` 回 `21;1`，加 `ESC[r` 无回包），据此定位会把面板顶到第 2 行并抹掉启动横幅。`crossterm::cursor::position` 本机恒返回 `Ok((0,0))`，不可用，改用自实现 `query_cursor_row`（`libc::poll` 限时 + 先排空 stdin 陈旧字节，失败返回 `None` → 保持吸底，不猜）。面板顶行经 `LAST_PANEL_TOP` 静态量传递，供 `teardown_pinned` 精确清除。
- **人工介入 `/hitl` 应急通道**：弹窗接管期间 TUI 此前**完全不读 stdin**，弹窗一旦屏外/被遮挡/进程卡住就是 120s 死等。现并联一条**只认 `/hitl` 前缀**的行读（`tui/hitl_view.rs::read_hitl_escape_answer` + `HumanAssistHub::respond_via_tui_escape` + 事件 `AssistEvent::TuiEscapeAnswered`）：`/hitl 2t6x` 文本应答、`/hitl 3` 选项 3、`/hitl cancel` 取消，映射规则与既有行读完全一致；**其它任意输入行丢弃并提示**，绝不与弹窗抢应答。
- **REPL 主屏**：保留 `InputHandler` 单行输入 + 斜杠命令补全 + 多轮对话。**粘贴保真**：bracketed paste 整体接收；**≥2 行粘贴一律转 `[粘贴 #N +M 行]` marker 保真**（原文含换行入登记表，不再把换行压成空格——旧「>10 行才转」规则会让 3~8 行 Markdown 提示词在屏幕上只剩一行、送进模型也丢掉列表结构），注册后立刻在滚动区回显原文预览（前 4 行 + 「其余 N 行未显示」），提交时按**展开版**逐行完整回显（首行 `>> `、续行 `.. `、超 20 视觉行折叠并标注「内容已完整发送」）；单份 >10000 字符截断为首尾各 500 + 省略标注+ 快速输入批量合并（IME/旧终端粘贴逐字重绘优化）+ Enter 提交前**三级粘贴判定**（：40ms 队列探测 → 击键节奏武装态延长观望 300ms → 正常提交；无 bracketed paste 的 Windows 终端多行提示词被逐行拆成多次提交的根治 — 见 `tui/input/mod.rs::drain_paste_burst` 与「无 bracketed paste 终端多行粘贴三缺陷根治」条目）。
- **子屏（Modal）**：`engine.rs` 的 Screen 栈接管 `/provider *` 系列，进入 alternate screen + 原始模式，Esc 退回主屏。
- **非 TTY 回退**：stdin 不是终端时（管道 / 重定向），子屏与主屏都回退到 print 输出。副作用是**子屏行为无法用 `cargo test` 覆盖**（见「自动化测试 · TUI 子屏仍需 tmux」）。

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
  `engine::present` 逐 cell 检测样式变化并输出 ANSI 序列。
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
- `docs/TUI自动化测试/` — TUI 子屏**人工**验证手法（tmux control-mode 真 PTY）；自动化载体已删，文首有作废抬头
- `docs/TUIMarkdown富文本渲染/` — TUI Markdown 渲染设计

**Agent 设计**：
- `docs/YoloAgent设计/` — Yolo 入口层设计（01，**唯一现行文档**：三档分类 / ReAct 信息收集 / 目标锚点三态 / 提示词编写原则与「无程序级拒绝门」说明）
- `docs/PlanAgent` / `docs/Main-Work工具扩展与ReAct改造/` — Main-Work 流程层 ReAct 化与工具扩展
- `docs/SubAgentWork执行层ReAct与连续工作模式/` — 执行层 ReAct 强化 + 工具连续工作模式 + 无进展止损
- `docs/Quality-Check Agent` / `docs/SessionContext Agent` / `docs/Debug模式与DebugAgent设计/` — 各角色设计

**协议与请求层**：
- `docs/Anthropic协议系统提示词三段式/` — 8 角色 system 三段式重构（billing + identity + rules）
- `docs/协议抓包/` — 各 Agent 真实 HTTP 抓包（RequestBody/ResponseBody）
- `docs/其他Agent工具定义/` — claude-code / codex / hermes / openclaw / open-code / pi / WorkBuddy 等的工具定义，新增工具时先读这里

**三个 MCP 风格工具**（按工具）：
- `docs/MCP_Window_Use/` — 桌面窗口操控（01 主设计 + 02 鼠标键盘优先级链 + 03 连续工作模式 + 04 中键修饰键 + 05 macos_legacy 拆分 + 06 横向对比 + MacOS/Window 平台技术文档）
- `docs/MCP_Web_Use/` — 浏览器操控（01 主设计 + 02 人工介入与窗口可视化 + 03 人工介入弹窗原生UI + **04 网页取证纪律** + **06 复用已登录浏览器会话** + **08 浏览器进程保护与凭证区域部分放行** + **09 自动阻断感知** + **10 验证弹窗处置闭环与执行超时兜底** + **11 人工核验弹窗输入硬闸与误报根治** + **12 产物锚定与人工核验闭环** + **13 页面管控三档 + locked 可滚动观看（guard 唯一最新版，已并入 05/07）** + **14 窗口铺满置前与弃权如实转达** + **15 输入链路根治与批量效率升级** + 16 内网站点实测缺陷修复（首启空标签页/空白页诊断/句柄自愈）+ **17 headed 视口铺满根治与主文档健康诊断** + **18 多标签页感知与后台挂起治理** + **19 select_dropdown 命中鲁棒性与 HITL 盾区几何精化(跨域 iframe @rect 精确挖洞/选择器唯一性自验/挑战弹层收窄)** + **20 select_dropdown 选项优先与选后确认闭环** + **21 屏蔽完整性与焦点租约(锁定态 CSS 层/LAEW_WEB_FOCUS 焦点租约)** + **22 多窗口并行双站同时工作(new_window 单进程多窗口+自动横向平铺防 occlusion 节流,LAEW_WEB_MAX_WINDOWS 上限)** + **23 站点频控节奏治理与登录墙误报降噪(rate_limited 横幅识别/同域冷却 6004/最小导航间隔/login 关键词不再中断 sequence)** + **24 屏蔽页面状态栏与进度栏(具体 Agent 名 + 动态进度,task_local 活动台账,closed Shadow)** + **26 反检测档位与非侵入式/侵入式模式设计(stealth 三档 fidelity/hardened/off + 五层技术栈取舍 + 描述符形态还原/toString 原生化/幂等闸)** + **27 Windows弹窗macOS风格界面美化(主题模块唯一真源/浅深配色板/自绘圆角按钮与macOS开关/DWM标题栏同色与圆角/进程窗口自遍历/LAEW_WIN_THEME 杀开关;新增 Windows 弹窗必走该模块) + **28 evalJs词法泄漏根治与selectDropdown确认闭环与HITL按钮白底修复(eval_sanitize二次整形 shield_toplevel_lexical/词表+正则动作型确认按钮/触发器重踢/draw_text_line TRANSPARENT)** + **29 反检测自洽性强化与屏幕几何泄漏根治(真机 A/B/C 三态差分复核：伪造 plugins/mimeTypes 改走真原型链、删凭空 chrome.runtime、屏幕几何 screen 声明补齐)** + **30 反检测环境自洽性强化(Notification 一致性精确映射/languages 去 freeze/迭代器自反)** + **31 反检测注入时序与跨档自洽增强(隐身脚本移到 goto 之前注册/幂等闸改块位掩码/跨档 `__adoptAll` 重新认领与迭代器重装/`Plugin.mimeTypes` 补齐/`automation_globals` 等四项漏检自证 + `risk_tier` 三档收敛)** + **32 反检测自证判据真机复核与首文档盲区根治(四档真机复核推翻三条自证：languages 冻结方向修反/focus_visibility 退出硬判据/injected_ns_keys 正则修真 + 首文档盲区根治(about:blank 起页,目标 URL 请求 2→1)/回填替身真机逐字段差分查出中间原型缺 own constructor/服务端侧取证冒烟)** + **33 注入面收敛与反检测自证升级(窗口面 30 个 `__lzn*` 字符串键→1 个 `Symbol.for` 命名空间对象/DOM 面独立第二 token/`render_ns` 后缀白名单三步改写/`injected_ns_keys`·`ns_symbol_present`·`outer_equals_inner` 自证/真机冒烟五项对账)**)。**05/07 已并入 13，文首有取代抬头，改 guard 相关代码只读 13；窗口/视口几何最新结论看 17。**
- `docs/Use_MCP/` — 通用 MCP 服务调用（Use_MCP，真 MCP 协议客户端）
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

**全部测试都是 Rust 代码级测试，统一用 `cargo test` 跑。** 原 shell 端到端脚本
(`testReport/run_e2e.sh` + `scripts/mock_llm_server.py`) 已整体删除。

| 层 | 入口 | 用途 |
| --- | ---- | ---- |
| 单元测试 | `cargo test --lib` | Rust 函数级覆盖（模块、解析、转换、工具） |
| 集成测试 | `cargo test --test <name>` | 跨模块的功能测试，见下表 |

### 集成测试布局（`tests/`）

| 文件 | 覆盖 |
| ---- | ---- |
| `tests/support/mod.rs` | **公共夹具**：进程内 mock LLM 服务（真实 HTTP/SSE，按协议出帧）、抓包模型、断言助手。取代原 `mock_llm_server.py` |
| `tests/protocol_wire.rs` | 双协议 wire 契约：端点补全、认证头、body 形态、tools schema 差异、`metadata.user_id`、`cache_control` 断点、逐角色 User-Agent |
| `tests/agent_loop.rs` | Agent 主循环功能：工具调用闭环、结果保序回填、Bash 危险命令 fail-closed、提示注入告警、ReAct 无进展止损、角色工具面收窄 |
| `tests/cli_provider.rs` | 真实二进制 CLI：`provider` / `mcp` 增删列切、导入导出、空库引导 |
| `tests/project_context.rs` | 项目说明文件五级发现链 + 注入幂等 |
| `tests/web_use_smoke.rs` | MCP_Web_Use 真浏览器冒烟（默认 `#[ignore]`，需本机装 Chrome） |
| `tests/stealth_reveal_smoke.rs` | 反检测的**服务端侧取证 + 回填替身真机差分**（独立进程跑，避免与 `web_use_smoke` 抢单例）：本地服务器记录 `User-Agent`/`Sec-CH-UA`/请求序号（首文档单一加载）、插件回填产物与同机原生逐字段对账 |
| `tests/macos_ax_smoke.rs` | macOS Accessibility 冒烟（依赖宿主 TCC 授权） |
| `tests/browser_watchdog.rs` | 浏览器子进程回收 |

### 写集成测试的规矩

1. **不打桩生产类型**。`protocol_wire.rs` 跑真实协议客户端打真实 HTTP，`agent_loop.rs`
   跑真实 `Agent` 循环与真实工具 —— mock 只决定「模型这一轮说什么」。
   打桩 `LlmClient` 只在**纯循环语义**（回填顺序、止损轮数）时才用。
2. **一律用绝对路径**。进程 cwd 是全局量，集成测试默认并行，依赖它会与 Bash 工具
   子进程的 `current_dir` 抢出竞态（同 `src/agent/` 单测里的 `GLOBAL_ENV_CWD_LOCK`）。
3. **CLI 用例先复制二进制到 `tempfile` 临时目录**再执行：二进制 SQLite 落在「程序目录」，
   复制即换库，天然隔离且不碰用户真实接入点。
4. **mock 端口交 OS 分配**（`127.0.0.1:0`），禁止固定端口 —— 原脚本 18901~18913 靠串行
   加 `kill;wait` 才没撞车。
5. **中文测试函数名**，与本仓库单测风格一致；`#![allow(dead_code)]` 只加在夹具模块顶部。
6. **断言失败要能自解释**：抓包类断言走 `support::dump_requests` / `assert_some_request`，
   失败时打印全部请求的 path / UA / 工具面。

### 单测与集成测试的分工

能在函数级判定的（解析、校验、纯函数、状态机）写 `src/**` 内的 `#[cfg(test)] mod tests`；
只有「跨模块串起来才有意义」的（wire 出参、Agent 循环、真实 CLI、真实文件系统布局）
才写 `tests/`。**不要为了覆盖率把函数级断言搬进集成测试**。

### TUI 子屏仍需 tmux（当前无自动化）

`src/tui/mod.rs::run` 按 `atty` 分流：TTY 走全交互，**非 TTY 子屏只 print 输出、不是真实渲染**。
因此 `/provider list` 等子屏行为**无法用 `cargo test` 覆盖**，需人工或临时 tmux 验证：

```bash
tmux new-session -d -s laew -x 100 -y 30 "./laew"
tmux send-keys -t laew -l "/provider list"; tmux send-keys -t laew Enter
SCREEN=$(tmux capture-pane -p -t laew); echo "$SCREEN" | grep -F "/provider list"
tmux kill-session -t laew
```

纯文本命令（`/help` `/model` `/new` `/exit`）可管道冒烟；横幅等宽、Markdown 渲染等
**纯渲染逻辑**应写成对 `tui::textfit` 等纯函数的单测，而不是靠抓屏比对。

## 约定

- 注释、CLI 文案、文档一律中文；代码标识符英文。
- **单源码文件 ≤ 1800 行**：超过必须按功能/业务/架构维度拆分模块；临界文件（≥ 1700 行）新增代码优先落到职责子模块，防止越线。**拆分规范**：
  - 单文件超线 → 转同名目录（`xxx.rs` → `xxx/mod.rs` + 职责子模块）；`mod.rs` 承载结构体定义 + 构造器/入口 + `pub use` 再导出，**外部 `crate::…::xxx::Yyy` 路径零改动**；
  - 原私有项搬入子模块标 `pub(super)`（可见域 = 本目录子树，与拆分前单文件作用域等价），由 `mod.rs` 私有 `use` 重导入供兄弟模块 `use super::*` 取用；
  - 代码**逐行机械搬移零改写**（不重构逻辑/注释），仅新增模块文档头与 `use super::*`；测试搬 `tests.rs`（mod.rs 声明 `#[cfg(test)] mod tests;`），拆分前后测试数必须对账一致；
  - 测试模块一律用 `xxx/tests.rs` **文件**（`mod tests;` 解析到文件）；某个 `tests.rs` 自身超线时，把新用例放到**同级兄弟文件**（如 `src/agent/react_tests.rs` + `#[cfg(test)] mod react_tests;`），不要转目录。注意与**包根 `tests/`（Cargo 集成测试目录）**区分：那里放跨模块集成测试，与本条的模块内单测是两回事；
  - 高速增长文件（近几轮每轮 +50 行以上）在破线前预防性拆分。已按此规范拆分：`src/tui/`（分层）、`src/agent/`（agent_loop + runtime_hints + tests 平铺拆分）、`agent/orchestrator/`、`agent/main_work/`、`agent/tools/window/`。
- 新工具：在 `src/agent/tools/` 建同名模块实现 `Tool` trait，注册进 `builtin_registry`（Work Agent）或相应 registry，Schema 参考 `docs/其他Agent工具定义/`。
- 新协议：实现 `LlmClient` trait + `client_from_record` 增加分支，不改动 agent 层。
- 新 Agent 类型：实现 `AgentProfile`（独立名称/系统提示词/工具集），在 `YoloRunner` 或相应编排器中接入。
- 临时计划放 `tmpPlan/`（已 gitignore）。测试产物（编译日志 `logs/build.log`、运行日志、调试报告等）同样不入库。
- `laew`、`*.db`、`tmpPlan/`、`target/` 均不入库（见 .gitignore）。

## Git 提交规范

**格式**：`<type>(<scope>): <中文业务描述>`，沿用仓库既有风格（可选轮次后缀，如 `feat(web): 可视化模式页面蒙层与人工操作拦截`）。
`type` 取 `feat` / `fix` / `perf` / `refactor` / `test` / `docs` / `chore`；`scope` 用模块名（`web` / `tui` / `llm` / `qc` 等）。描述只写**做了什么业务改动**，不写过程与情绪。

**硬性禁止（生成 commit message 时一律不得出现）**：

1. 禁止添加任何 `Co-Authored-By: Claude Code <noreply@anthropic.com>` 行（以及其它形式的 AI 联合作者署名）。
2. 禁止追加 `Generated with Claude Code` 这类 AI 署名/生成声明文本（以及 `🤖 Generated with …` 等变体）。
3. commit message **只保留业务描述**，末尾不追加任何 AI 相关的 attribution 尾部注释（不写「本提交由 AI 生成」「由 Claude Code 协助」等）。

**其它约定**：

- 默认**不主动提交**：改完代码先向用户汇报 diff 摘要，等明确指示再 `git commit`。
- 需要提交时先确认在 `main` 之外的分支（默认分支上先建分支）。
- 提交前确保 `cargo build` 通过；改动范围大时同步跑 `cargo test`。