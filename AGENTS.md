# AGENTS.md — LsmAgentEmergentWork

供 AI Agent Tools（Claude Code / Codex / Hermes / OpenCode / pi / OpenClaw 等）自动加载的工程入口说明。

## 工程是什么

由 LLM 驱动的 Rust Agent CLI（二进制名 **`laew`**）。支持 Anthropic（anthropic-messages）与
OpenAI（openai-completions）双协议，**双 Agent 架构**（Yolo 入口层 + Work 执行层），
内置 Bash / Read / Write 三个工具，TUI 多轮对话 + `-p` 单轮模式 + `-f` 文件提示词模式。
TUI 支持斜杠命令自动补全（Tab 补全 + 行内提示）和文件路径补全。
配置持久化在 **根目录** SQLite（`LsmAgentEmergentWork.db`），不使用配置文件。

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
./laew provider add|list|use|delete ...
```

注意：crates.io 在本机网络较慢，已在 `~/.cargo/config.toml` 配置 rsproxy.cn 镜像。

## 领域概念（改代码前必读）

- **根目录** = `laew` 二进制所在目录（`current_exe()` 父目录）。数据库 `LsmAgentEmergentWork.db`、编译产物 `./laew` 都在这里。
- **工作目录** = 启动命令时所在目录。Bash/Read/Write 工具的相对路径基准。两者可能不同，勿混淆。
- **当前项目说明文件** = 以**工作目录**为基准按五级链发现：非空 `CLAUDE.md` → 非空 `AGENTS.md` → 非空 `README.md` →（都没有但根目录层有其它 `*.md` 时，程序化分析后**自动生成 `README.md`** 落盘使用）→ 空（不注入）。Yolo 在每个 Session **首次处理**时，把「工作目录路径 + 说明文件内容」包装成带 `<<<LAEW:PROJECT_CONTEXT>>>` 标记的独立 user 消息插入上下文 index 0（标记探测幂等、与用户提示词严格隔离），设计见 `docs/Yolo项目上下文注入/`。TUI 横幅的「项目说明:」行为纯探测展示。
- **接入记录（完整的大模型接入记录）** = `protocol(anthropic|openai) + provider_name + model_name + end_point + api_key` 五元组，存 SQLite `providers` 表，可多条，`is_active` 唯一。
- **接入点补全**：Anthropic → `{end_point}/v1/messages`；OpenAI → `{end_point}/chat/completions`；尾部 `/` 自动裁剪。
- **工具定义协议差异**：Anthropic 用 `tools[].{name,description,input_schema}`；OpenAI 用 `tools[].{type:"function",function:{name,description,parameters}}`（function 风格）。
- **双 Agent 架构**：
  - **Yolo Agent**（`LsmAgentEmergentWork-Yolo`）：入口层，负责目标识别 / 意图识别（每条输入先做 目的→目标→意图 三步分析）/ 任务四级分类（trivial/simple/medium/hard），中高难度先做拆解；仅持 Read 工具；每会话首次处理前注入项目上下文。
  - **Work Agent**（`LsmAgentEmergentWork-Work`）：执行层，持 Bash/Read/Write 全套工具，实际干活。
  - 两者均为标准 `AgentProfile`，通过 `YoloRunner` 编排：用户输入 → Yolo 分类 → 直接回答 / 委派 Work。
- **AgentProfile**：Agent 身份档案（名称 / 系统提示词 / 工具集），`work_profile()` / `yolo_profile()` 两个工厂函数。
- **Session**：进程内会话，拥有独立 Session ID 与对话上下文（context）；TUI 启动或 `/new` `/clear` 时生成新 Session。
- **请求头**：两协议统一携带 `User-Agent: {AgentName}/{版本} {编译时间}`、`Authorization: Bearer {api_key}`、`X-Session-Id`；Anthropic 请求体 additionally 携带 `metadata.user_id`（含 `device_id/account_uuid/session_id`）。

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

| 命令 | 行为 |
|------|------|
| `/help` (h, ?) | 显示帮助 |
| `/exit` (quit, q) | 退出 TUI |
| `/clear` (c) | 清空对话历史，开启新 Session |
| `/new` (n) | 同 `/clear`（开启新 Session） |
| `/model` | 显示当前模型 |
| `/provider` | 管理接入记录（默认进入 list 屏） |

### `/provider` 系列交互

| 输入 | 行为 |
|------|------|
| `/provider` | **默认等价 `/provider list`**，进入 ProviderList 屏 |
| `/provider list` (ls) | ProviderList 屏：5 只读字段 Tab + 操作按钮（设为当前 / 删除 / 返回） |
| `/provider add` | ProviderForm 屏：5+1 Tab 表单（protocol / provider_name / model_name / end_point / api_key / 确认） |
| `/provider use <id>` | 后台 set_active + 重建 Agent，回到主屏打印 `✓ 已切换` |
| `/provider del` | ProviderDelPicker 屏 → ProviderDelConfirm 屏（二次确认） |

### Tab 表单交互（ProviderForm）

- 5 个数据 Tab + 1 个确认 Tab；`←` / `→` 环回切换；`Enter` 进入编辑态。
- protocol Tab：Enter 切换 `anthropic ⇄ openai`。
- 文本 Tab：进入行输入，Enter / Esc 退出编辑态（保留修改）。
- 确认 Tab：`[ 确认 ]` / `[ 取消 ]`，左右切换，Enter 触发。
- **API Key 全程脱敏**：浏览态显示 `****<末4位>`；仅进入 Tab 5 编辑态时显示明文。

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
- `docs/协议抓包/` — 各 Agent 真实 HTTP 抓包（RequestBody/ResponseBody）。**codex 走 responses 接口仅参考请求**，其余为主要参考
- `docs/其他Agent工具定义/` — claude-code / codex / hermes / openclaw / open-code / pi / WorkBuddy 等的工具定义，新增工具时先读这里
- `docs/Agent源码调研/` — **15 个外部项目源码**的系统调研与深度分析,**共 45 份文档/~27k 行**（15 份综合文档 + 30 份横向专题）。原始 101 份多轮文档（~138k 行）已于 2026-09-06 合并为单一综合文档，删除跨轮次重复内容。覆盖架构/多轮对话/Context/循环架构/工具调用/记忆系统/Workflow/目标意图识别/目标规划/Agent协作调度/Yolo/质检/任务拆解/分类/MCP/SKILL/沙箱设计/权限管控/LLM网关/协议翻译/上下文注入/决策溯源/HTTP客户端/协议调用实现/Agent间通信协议等 27+ 维度。

  **15 份 Agent 综合文档**（每份合并了源码调研/深度分析/核心机制/第二轮/第三轮/第四轮共 3-8 轮内容，去重压缩 50-90%）：
  | 文件 | 语言/技术 | 核心亮点 | 行数 |
  |------|----------|---------|------|
  | `atomcode.md` | Rust | L0/L1/L2 分层 + cargo feature gating + kernel 内部 trait + MCP 7 子模块 + CodeIntel 七件套 + daemon + tuix TUI | ~2,047 行 |
  | `claudecode.md` | TypeScript/Bun | 四级压缩管线 + 27 种 Hook + 5 种执行器 + 40+ 工具 + Ink Fork + Bridge 远程控制 | ~1,006 行 |
  | `deepseek-harness.md` | TypeScript | Cordis Everything-is-a-Plugin + Fiber epoch + 30+ 核心模块 + Typert 协议 + ACP/A2A/E2A/A2UI | ~1,116 行 |
  | `openclaw.md` | TypeScript | Gateway/Harness/Adapter 三层契约 + 162 extensions + 双向 MCP + Lane 调度器 + Workshop 自演化 | ~888 行 |
  | `opencode.md` | TypeScript/Bun | Effect + Schema 全栈 DI + LayerNode 拓扑 + 34 包 + enterprise Durable Object + R2 | ~936 行 |
  | `pi.md` | TypeScript | lane 并发 + 一等公民 Skill + 二进制帧协议(CBOR) + WriterLease fence + 14 种损坏检测 | ~1,319 行 |
  | `hermes-agent.md` | Python | 859 MB + 6 前端共享 AIAgent + 38 provider + CompressionCommitFence + FTS5 + Trigram | ~1,632 行 |
  | `undici.md` | JavaScript | Node.js 官方 HTTP 客户端(非 Agent) + Dispatcher + HTTP/2 + llhttp WASM + 8 拦截器 + Mock 录制回放 | ~7,639 行 |
  | `cc-switch.md` | Tauri 2+Rust+React | 8 款工具适配 + 熔断器三态 + thinking_rectifier + MCP SSOT + 17 schema 迁移 + WebDAV/S3 | ~1,293 行 |
  | `agent-core.md` | Python | openJiuwen Core SDK + ReAct + ContextEngine + 多类型记忆 + PermissionEngine + Pregel + Rails + OTLP | ~1,630 行 |
  | `agent-studio.md` | Python | 一站式 Agent 平台 + Pregel cba 消减 + DSL 双向转换 + 5 种 MCP 传输 + BubbleWrap + Seccomp | ~1,005 行 |
  | `jiuwenswarm.md` | Python | 多 Agent 协作 + Leader-Teammate + A2A/ACP/E2A/A2UI + SkillDevPipeline 12 阶段 + SwarmFlow DAG + JiuwenBox | ~1,395 行 |
  | `semantica.md` | Python | 图原生 AI 基础设施 + Context Graph + Rete + Datalog + SPARQL + W3C PROV-O + BiTemporalFact | ~1,716 行 |
  | `Switchyard.md` | Rust | NVIDIA LLM 网关 + 协议 IR + ContentBlock::Unknown + TranslationEngine + 7 种路由算法 + PyO3 | ~1,807 行 |
  | `TencentDB-Agent-Memory.md` | TypeScript+Python | 团队记忆系统 + L0-L3 管线 + SkillCore 6写4读 + InjectionPipeline 8注入点 + RRF 混合检索 | ~1,569 行 |

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

- `docs/Agent架构对比与参考.md` — 7 个项目(6 外部 + laew)的横向对比报告,含 10 维度对比表、15 个跨项目设计模式、laew 借鉴路线图(P0/P1/P2)、反模式警示

## 自动化测试

测试分三层,放在 `testReport/` 下:

| 层 | 入口 | 用途 |
|----|------|------|
| 单元测试 | `cargo test` | Rust 函数级覆盖(模块、解析、转换、工具) |
| 端到端(CLI) | `bash testReport/run_e2e.sh` | mock LLM,跑 `-p` / `provider add|list|use|delete` / 协议 wire 校验 / **项目上下文注入(说明文件五级链,5b 节)** / TUI 管道冒烟 / **TUI 子屏 tmux 自动化** |
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
