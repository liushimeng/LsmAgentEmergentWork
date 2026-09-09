# AGENTS.md — LsmAgentEmergentWork

供 AI Agent Tools（Claude Code / Codex / Hermes / OpenCode / pi / OpenClaw 等）自动加载的工程入口说明。

## 工程是什么

由 LLM 驱动的 Rust Agent CLI（二进制名 **`laew`**）。支持 Anthropic（anthropic-messages）与
OpenAI（openai-completions）双协议，**多 Agent 架构**（6 角色 + 三档难度），
内置 Bash / Read / Write 三个工具，TUI 多轮对话 + `-p` 单轮模式 + `-f` 文件提示词模式。
TUI 支持斜杠命令自动补全（Tab 补全 + 行内提示）和文件路径补全。
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
./laew -debug [-p "任务" | -f prompt.md]  # 调试模式(等价 --debug):采集各 Agent 输入输出/性能/质量,任务后由 Debug Agent 评估并生成报告
./laew provider add|list|use|delete ...
```

注意：crates.io 在本机网络较慢，已在 `~/.cargo/config.toml` 配置 rsproxy.cn 镜像。

## 领域概念（改代码前必读）

- **根目录** = `laew` 二进制所在目录（`current_exe()` 父目录）。数据库 `LsmAgentEmergentWork.db`、编译产物 `./laew` 都在这里。
- **工作目录** = 启动命令时所在目录。Bash/Read/Write 工具的相对路径基准。两者可能不同，勿混淆。
- **当前项目说明文件** = 以**工作目录**为基准按五级链发现：非空 `CLAUDE.md` → 非空 `AGENTS.md` → 非空 `README.md` →（都没有但根目录层有其它 `*.md` 时，程序化分析后**自动生成 `README.md`** 落盘使用）→ 空（不注入）。Yolo 在每个 Session **首次处理**时，把「工作目录路径 + 说明文件内容」包装成带 `<<<LAEW:PROJECT_CONTEXT>>>` 标记的独立 user 消息插入上下文 index 0（标记探测幂等、与用户提示词严格隔离），设计见 `docs/Yolo项目上下文注入/`。TUI 横幅的「项目说明:」行为纯探测展示。
- **接入记录（完整的大模型接入记录）** = `protocol(anthropic|openai) + provider_name + model_name + end_point + api_key` 五元组 + `context_max_size`(上下文最大 Token 数,默认 800K,`0` = 不限制/关闭自动压缩；支持 `800000`/`800K`/`1M` 写法)，存 SQLite `providers` 表，可多条，`is_active` 唯一。存量库打开时自动迁移补列回填默认值。
- **接入点补全**：Anthropic → `{end_point}/v1/messages`；OpenAI → `{end_point}/chat/completions`；尾部 `/` 自动裁剪。
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
  - 由 `MultiAgentOrchestrator` 总编排:用户输入 → 项目上下文注入 → Yolo 分类 → 简单档(SubAgent) / 中档(Main→SubAgent) / 高档(Plan→Main→SubAgent) → Quality-Check → SessionContext 收口。WorkFlow 执行时按 `depends_on` 自动 Kahn 分层(`main_work::topo_layers`),**同层无依赖的 SubAgent 自动并行**(tokio::spawn + Semaphore 上限 3,`OrchestratorConfig::max_parallel_workflows`),跨层严格串行、上游产物按层注入,失败语义与串行一致(fail-fast 回流 Yolo)。
- **Agent-Context / Agent-Memory**：
  - **Agent-Context**：每个 Agent 独立的实时上下文(消息流 + 状态)，内存态，生命周期 = 当前单元。
  - **Agent-Memory**：每个 Agent 独立的记忆层(输入/输出/错误/产物摘要)，持久化到 SQLite `agent_memory` 表，跨单元/跨 Session 复用。
  - 与 Session 主上下文(用户对话历史)严格隔离。
- **SessionContext 摘要**：每个用户任务完成后 SessionContext 生成 Markdown 摘要写入 `session_memory` 表；Yolo 下次处理时自动注入最近 N 条历史摘要(默认 3),用 `<<<LAEW:SESSION_HISTORY>>>` 标记隔离。
- **AgentProfile**：Agent 身份档案（名称 / 系统提示词 / 工具集），`work_profile()` / `yolo_profile()` 两个工厂函数。
- **Session**：进程内会话，拥有独立 Session ID 与对话上下文（context）；TUI 启动或 `/new` `/clear` 时生成新 Session。
- **请求头**：两协议统一携带 `User-Agent: {AgentName}/{版本} {编译时间}`、`Authorization: Bearer {api_key}`、`X-Session-Id`；Anthropic 请求体 additionally 携带 `metadata.user_id`（含 `device_id/account_uuid/session_id/agent`）。**User-Agent 按"发起请求的 Agent 角色"逐请求注入**（`Agent::run_session_inner` 从 profile 写入 `RequestMeta.user_agent`，8 角色各自携带自身名称，抓包层面可辨识；空值回退客户端构造期默认 UA）——见 `tmpPlan/2026-09-09_08` 方案。

## 架构（src/）

```
main.rs        clap CLI:默认进 tui; -p 单轮; -f 文件提示词; provider 子命令
tui/
  mod.rs       会话编排:REPL 主屏循环 + Screen 栈;暴露 pub async fn run()
  engine.rs    CLI 渲染引擎 —— Screen trait + Frame + 全量重绘 present
  form.rs      通用 Tab 表单状态机(被 ProviderForm 屏复用)
  input.rs     单行输入(主屏用):含行内提示 + 补全(crossterm 原始模式)
  completion.rs 斜杠命令补全引擎
  theme.rs     ANSI 颜色 / mask_key 脱敏 集中管理
  screen/
    provider_list.rs   /provider list —— Tab 化展示 + 操作按钮
    provider_form.rs   /provider add  —— 5+1 Tab 表单
    provider_del.rs    /provider del  —— Picker + 二次确认
agent/
  mod.rs       协议无关循环:run_session(Session) → complete → tool_calls → 执行 → tool_result 回填
  profile.rs   AgentProfile(名称 / 系统提示词 / 工具集) + work_profile()/yolo_profile() + User-Agent
  system_prompt/mod.rs  SystemPrompt 组合与渲染(基础 + 工具说明 + 协议尾缀)
  tools/
    mod.rs     Tool trait + ToolRegistry(有序) + builtin_registry()/yolo_registry()
    bash.rs    BashTool
    read.rs    ReadTool
    write.rs   WriteTool
  yolo.rs      YoloRunner 双 Agent 编排器 + TaskLevel + TaskClassification + JSON 解析
  compact.rs   CompactRunner:token 估算 / 三档选档 / 自动压缩触发 / 硬截断降级 / 保护段识别
  overflow.rs  上下文溢出检测(15+ provider 正则)+ 三级恢复(排水/折叠/暴露)
  project_context.rs 项目说明文件五级链发现 + README 自动生成 + 每会话首次注入(幂等标记)
session.rs       Session:本机指纹 device_id + Session ID 生成 + 独立对话上下文 context
llm/mod.rs       统一消息模型 + LlmClient trait + RequestMeta + build_common_headers
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

- **REPL 主屏**：保留 0.1.2 的 `InputHandler` 单行输入 + 斜杠命令补全 + 多轮对话。
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
| `/provider`       | 管理接入记录（默认进入 list 屏） |

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
- `docs/Context设置与自动压缩设计/` — ContextMaxSize 上下文上限(默认 800K,DB 迁移自动补全)+ Compact Agent(第 8 角色)三档自动压缩 设计与解决方案
- `docs/协议抓包/` — 各 Agent 真实 HTTP 抓包（RequestBody/ResponseBody）。**codex 走 responses 接口仅参考请求**，其余为主要参考
- `docs/其他Agent工具定义/` — claude-code / codex / hermes / openclaw / open-code / pi / WorkBuddy 等的工具定义，新增工具时先读这里
- `docs/Agent源码调研/` — **15 个外部项目源码**的系统调研与深度分析，**共 80+ 份文档/约 168k 行**（15 份综合文档 + 55 份横向专题；按轮次组织，主文档每轮追加新章节，专题目录按主题持续扩容）。每轮合集见 `专题/专题-第N轮深挖合集.md`。
  - **2026-09-09 第十九轮（当前最新）**：6 份全新专题（**~7,700+ 行 / ~450 KB**）—— 7 工程（atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / undici）主文档各追加一章 + 6 份专题文档 + 1 份跨项目缺口分析。聚焦前 18 轮未覆盖的「**用户交互体验层续 + 安全纵深 + 多模态 + A2A + a11y + 离线 + 同步**」6 大全新维度：D9 安全与威胁模型（STRIDE / Prompt 注入 / Bash 检测 / 凭证 / SSRF / 沙箱 / 审计）/ D10 多模态输出（图表 / Mermaid / 图片协议 / 数学公式 / HTML / SVG）/ D11 A2A 协议（A2A/ACP/E2A/A2UI/MCP 双向/跨语言）/ D12 可访问性 a11y（屏幕阅读器 / 高对比主题 / 减动效 / RTL / 键盘可达）/ D13 离线模式（离线检测 / 请求队列 / 本地缓存 / 同步合并）/ D14 跨设备同步（设备发现 / 配对 / 同步协议 / 加密 / 状态合并）。新增 laew gap: L1591-L1930+（340+ 个）。合集见 `专题/专题-第十九轮深挖合集.md`。累计 100+ 维度（90+ 基础设施与协议层 + 16+ 用户交互体验层）。**第二十轮候选**：国际化 i18n 完整实现 / Web UI + Desktop App / OAuth 认证与多账号 / Release 工程化与 AutoUpdate / DevContainer 与容器化 / CRDT 与多端冲突。
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
- 新工具：在 `src/agent/tools/` 建同名模块实现 `Tool` trait，注册进 `builtin_registry()`（Work Agent）或相应 registry，Schema 参考 `docs/其他Agent工具定义/`。
- 新协议：实现 `LlmClient` trait + `client_from_record()` 增加分支，不改动 agent 层。
- 新 Agent 类型：实现 `AgentProfile`（独立名称/系统提示词/工具集），在 `YoloRunner` 或相应编排器中接入。
- 测试报告输出到 `testReport/`（命名 `e2e-<时间戳>.txt` / `验证报告-<日期>.md`）；临时计划放 `tmpPlan/`（已 gitignore）。
- `laew`、`*.db`、`tmpPlan/`、`target/` 均不入库（见 .gitignore）。
