<!-- markdownlint-disable -->
<div align="center">

[🇨🇳 中文](README.md) · [🇬🇧 English](README.en.md) · [🇯🇵 日本語](README.ja.md)

---

```
╔══════════════════════════════════════════════════════════════════════╗
║                                                                    ║
║        ██╗      █████╗ ███████╗██╗    ██╗                          ║
║        ██║     ██╔══██╗██╔════╝██║    ██║                          ║
║        ██║     ███████║█████╗  ██║ █╗ ██║                          ║
║        ██║     ██╔══██║██╔══╝  ██║███╗██║                          ║
║        ███████╗██║  ██║███████╗╚███╔███╔╝                          ║
║        ╚══════╝╚═╝  ╚═╝╚══════╝ ╚══╝╚══╝                           ║
║                                                                    ║
║        LLM · Agent · CLI · Rust · Multi-Agent · 6 Roles           ║
║        双协议 · 6 工具 · 6 角色编排 · TUI · SQLite                 ║
║                                                                    ║
╚══════════════════════════════════════════════════════════════════════╝
```

## 🦀 Rust 多 Agent CLI · 双协议 · 6 角色同台编排

</div>
<!-- markdownlint-restore -->

<div align="center">

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork)
[![AI Agent Coded](https://img.shields.io/badge/AI--Agent-100%25-ff6b6b)](CLAUDE.md)
[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![Chinese](https://img.shields.io/badge/lang-中文-red)](README.md) [![English](https://img.shields.io/badge/lang-English-blue)](README.en.md) [![日本語](https://img.shields.io/badge/lang-日本語-green)](README.ja.md)

</div>

---

> **🤖 100% AI Agent 自动编程** —— 无人工手写一行代码，无古法编程。
> 整个项目（Rust 源码、多 Agent 架构、TUI 渲染引擎、工具系统、自动化测试、CI 脚本、文档）
> 全部由 AI Agent（Claude Code 等）自主编写、编译、测试、重构、部署。
>
> **⏱️ 14 轮深度调研 · 82+ 维度 · 635 个 gap 知识库** —— 本仓库不是「一次性会话产物」，
> 而是 Agent 持续深挖 14 轮、覆盖 82+ 维度、沉淀 635 个 laew gap 的「Agent 编程」能力完整展示。
> 每一轮都由 Agent 自动读取上一轮产出、规划新维度、产出专题报告、写入知识库、git 提交。

---

## 🤖 100% Agent 自动编程 —— 本仓库的核心亮点

> **这不是一个人写的代码。这是一群 AI Agent 24 小时不间断编程的作品。**

本仓库的所有代码均由 AI Agent 自动编写：

- **无手工编码**：没有人类程序员手写任何一行 Rust / TOML / Shell / Markdown。
- **Agent 协作**：按职责拆分为多条 SubAgent 职责线（架构、TUI、工具、协议、测试、文档），各自独立工作、自动提交。
- **自测自修**：Agent 自动运行 `cargo test`、`bash testReport/run_e2e.sh`，发现 Bug 后自动定位、修复、回归验证。
- **自部署**：`rebuild_restart_app.sh` 由 Agent 编写，一键 `cargo build --release` → 拷贝 `./laew` → 重启。
- **持续迭代**：`CLAUDE.md` 中记录了海量 Agent 教训，每条都是 Agent 踩坑后自动写入的「经验记忆」，后续 Agent 自动加载避免重犯。
- **知识库沉淀**：`docs/` 下 80+ 份调研文档 / 约 160k 行，全部由 Agent 自动产出。

> **仓库数据**：Rust 多 Agent CLI、6 工具、6 角色编排、双协议、TUI 子屏自动化。
> **这一切，没有一个人工手写字符。**

---

## 🎯 项目定位与核心能力

`laew`（**L**lm **A**gent **E**mergent **W**ork）是一款基于 Rust 的 LLM 多 Agent CLI，
支持 **Anthropic** 与 **OpenAI** 双协议，内置 6 个工具调用，提供 TUI 多轮对话、
`-p` 单轮任务、`-f` 文件提示词三种模式。

| 能力 | 说明 |
|------|------|
| 🧠 **双协议** | Anthropic（anthropic-messages）+ OpenAI（openai-completions），统一消息模型隔离协议差异 |
| 🛠️ **6 工具** | Bash / Read / Write / Edit / Glob / Grep |
| 🤖 **6 角色多 Agent** | Yolo / Plan / Main-Work / SubAgent-Work / Quality-Check / SessionContext |
| 📊 **三档分类** | simple / medium / hard，Yolo 自动分类后分层编排 |
| 🖥️ **TUI** | crossterm 独立渲染引擎，alternate screen + raw mode + Screen 栈 + Tab 表单 |
| 💾 **SQLite 持久化** | 根目录 `LsmAgentEmergentWork.db`，无配置文件 |
| 🔍 **项目上下文注入** | 五级链 CLAUDE.md → AGENTS.md → README.md → 自动生成 → 空，每会话首次幂等注入 |

---

## 🤖 多 Agent 架构 —— 6 角色同台编排

由 `MultiAgentOrchestrator` 总编排：用户输入 → 项目上下文注入 → Yolo 三档分类 →
简单档（SubAgent）/ 中档（Main → SubAgent）/ 高档（Plan → Main → SubAgent）
→ Quality-Check → SessionContext 收口。

| 角色 | 职责 | 工具 |
|------|------|------|
| 🎯 **Yolo Agent** | 入口层：目的→目标→意图三步分析 / 三档分类 / 失败回流 | Read |
| 🗺️ **Plan Agent** | 规划层：hard 任务输出 Markdown 方案到 `plans/` | Read / Write |
| 🔄 **Main-Work Agent** | 流程层：拆 WorkFlow 列表 | Bash / Read |
| ⚡ **SubAgent-Work Agent** | 执行层最小单元：每个流程委派一个 SubAgent | Bash / Read / Write |
| ✅ **Quality-Check Agent** | 质检层：每个执行单元完成后必经 QC | 可选 Read |
| 🧠 **SessionContext Agent** | 会话层：任务完成后汇总写 `session_memory` | 无工具 |

### 编排拓扑

```text
用户输入
   │
   ▼
项目上下文注入（五级链，幂等）
   │
   ▼
Yolo ──→ 分类: simple / medium / hard
   │
   ├─ simple ──→ SubAgent-Work ──→ Quality-Check ──→ SessionContext
   │
   ├─ medium  ──→ Main-Work ──→ SubAgent-Work ──→ Quality-Check ──→ SessionContext
   │
   └─ hard    ──→ Plan ──→ Main-Work ──→ SubAgent-Work ──→ Quality-Check ──→ SessionContext
```

---

## ⚙️ 技术栈

| 模块 | 选型 |
|------|------|
| 🦀 语言 | Rust 1.75+ |
| ⌨️ TUI | crossterm（独立 CLI 渲染引擎：Screen trait + Frame + alternate screen + raw mode） |
| 📟 CLI | clap（derive 风格：TUI / `-p` / `-f` / `provider` 子命令） |
| 💾 数据库 | rusqlite（SQLite，`LsmAgentEmergentWork.db`） |
| 🌐 HTTP | reqwest（Anthropic / OpenAI 双协议客户端） |
| 📡 协议 | Anthropic Messages + OpenAI Chat Completions（统一消息模型 `llm/mod.rs`） |
| 🧪 测试 | `cargo test` 单元 + `testReport/run_e2e.sh` 端到端（mock LLM + tmux 子屏自动化） |

---

## 🚀 快速启动

### 前置要求

- Rust 1.75+（推荐用 [rustup](https://rustup.rs)）
- Linux / macOS（TUI 依赖 crossterm 原始模式）

### 安装与部署

```bash
# 1. 克隆仓库
git clone https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork.git
cd LsmAgentEmergentWork

# 2. 一键编译（cargo build --release → 拷贝 ./laew 到根目录）
./rebuild_restart_app.sh

# 3. 查看版本
./laew --version
# laew 0.1.0 (build 2026-09-08 xx:xx:xx CST, git xxxxxxx)
```

### 配置大模型接入

```bash
# 添加一条 Anthropic 接入记录（五元组：protocol + provider_name + model_name + end_point + api_key）
./laew provider add --protocol anthropic \
    --provider-name myAnthropic --model-name claude-sonnet-5 \
    --end-point https://api.anthropic.com --api-key sk-ant-xxxx

# 添加一条 OpenAI 接入记录
./laew provider add --protocol openai \
    --provider-name myOpenAI --model-name gpt-5 \
    --end-point https://api.openai.com --api-key sk-xxxx

./laew provider list          # 查看（* 为当前使用）
./laew provider use 2         # 切换当前模型
./laew provider delete 2      # 删除记录
```

> 接入点自动补全：Anthropic 自动拼接 `v1/messages`，OpenAI 自动拼接 `chat/completions`。
> 配置不落配置文件，全部存于 **根目录** 的 SQLite（`LsmAgentEmergentWork.db`）。

> **自签名证书 / 内网网关**：`end_point` 为 HTTPS + IP 地址（自签名证书常见形态）时自动跳过证书校验，
> 开箱即用；域名自签名服务可设 `LAEW_TLS_INSECURE=1` 全局放行，设 `0` 强制全局严格。
> 仅跳过证书校验，TLS 加密不降级；rustls 纯 Rust 实现，Windows / macOS / Linux 行为一致。
> 设计见 `docs/自签名证书TLS适配/01-设计与解决方案.md`。私网/loopback 地址默认被 SSRF 防护拦截，
> 本地调试可设 `LAEW_ALLOW_PRIVATE_ENDPOINT=1` 放行。

### 三种使用模式

```bash
./laew                                  # 进入 TUI 多轮对话（横幅含根目录/工作目录/当前模型）
./laew -p "帮我看看当前目录有什么文件"     # 单轮任务模式
./laew -f /path/to/prompt.md            # 从文件读取提示词执行（支持绝对/相对路径）
```

### TUI 斜杠命令

| 命令 | 行为 |
|------|------|
| `/help` (h, ?) | 显示帮助 |
| `/exit` (quit, q) | 退出 TUI |
| `/clear` (c) / `/new` (n) | 清空对话历史，开启新 Session |
| `/model` | 显示当前模型 |
| `/provider` | 管理接入记录（默认进入 list 屏） |

---

## 🔑 关键概念

| 概念 | 说明 |
|------|------|
| **根目录** | `laew` 二进制所在目录；数据库与编译产物落在这里 |
| **工作目录** | 启动 `laew` 时所在目录；Agent 读写文件 / 执行命令的默认上下文 |
| **接入记录** | protocol + provider_name + model_name + end_point + api_key 五元组，可配多条，仅一条激活 |
| **接入点补全** | Anthropic → `{end_point}/v1/messages`；OpenAI → `{end_point}/chat/completions`；尾部 `/` 自动裁剪 |
| **协议差异** | Anthropic 用 `tools[].{name,description,input_schema}`；OpenAI 用 `tools[].{type:"function",function:{name,description,parameters}}` |
| **Agent-Context** | 每个 Agent 独立的实时上下文（消息流 + 状态），内存态，生命周期 = 当前单元 |
| **Agent-Memory** | 每个 Agent 独立的记忆层，持久化到 SQLite `agent_memory` 表，跨单元/跨 Session 复用 |
| **SessionContext 摘要** | 每任务完成后生成 Markdown 摘要写 `session_memory`，Yolo 下次自动注入最近 N 条 |

---

## 📁 项目结构

```
LsmAgentEmergentWork/
├── src/
│   ├── main.rs              # CLI 入口 (clap): TUI / -p / -f / provider 子命令
│   ├── lib.rs               # 库导出
│   ├── session.rs           # Session: 本机指纹 + Session ID + 独立对话上下文
│   ├── error.rs             # 统一错误类型 (thiserror)
│   ├── build.rs             # 注入 LAEW_BUILD_TIME / LAEW_GIT_HASH
│   ├── config/              # 根目录/工作目录解析 + SQLite CRUD
│   ├── database/            # schema / models / paths / provider
│   ├── agent/
│   │   ├── mod.rs           # 协议无关循环: run_session → complete → tool_calls
│   │   ├── orchestrator.rs  # MultiAgentOrchestrator 总编排
│   │   ├── yolo.rs          # YoloRunner 双 Agent 编排 + 三档分类
│   │   ├── plan.rs          # Plan Agent
│   │   ├── main_work.rs     # Main-Work Agent
│   │   ├── subagent.rs      # SubAgent-Work Agent
│   │   ├── quality.rs       # Quality-Check Agent
│   │   ├── session_context.rs # SessionContext Agent
│   │   ├── profile.rs       # AgentProfile + work_profile() / yolo_profile()
│   │   ├── context.rs       # Agent-Context 独立实时上下文
│   │   ├── memory.rs        # Agent-Memory 持久化记忆
│   │   ├── json_repair.rs   # JSON 自动修复链
│   │   ├── project_context.rs # 项目说明文件五级链发现 + 每会话首次注入
│   │   ├── system_prompt/   # SystemPrompt 组合与渲染
│   │   ├── permissions/     # 权限管控: dangerous / sensitive
│   │   ├── sandbox_hook/    # 沙箱钩子
│   │   └── tools/           # Tool trait + ToolRegistry + 6 个工具
│   ├── llm/
│   │   ├── mod.rs           # 统一消息模型 + LlmClient trait
│   │   ├── anthropic.rs     # Anthropic wire 转换
│   │   ├── openai.rs        # OpenAI wire 转换
│   │   ├── sse.rs           # SSE 流式解析
│   │   └── resilient.rs     # LLM 调用自动弹性层
│   └── tui/
│       ├── mod.rs           # REPL 主屏循环 + Screen 栈
│       ├── engine.rs        # CLI 渲染引擎: Screen trait + Frame
│       ├── form.rs          # 通用 Tab 表单状态机
│       ├── input.rs         # 单行输入 + 行内提示 + 补全
│       ├── completion.rs    # 斜杠命令补全引擎
│       ├── theme.rs         # ANSI 颜色 / mask_key 脱敏
│       └── screen/          # ProviderList / ProviderForm / ProviderDel 子屏
├── docs/                    # 知识库: 14 轮深度调研 / 82+ 维度 / 635 gap
├── testReport/              # 自动化测试报告 + run_e2e.sh
├── tmpPlan/                 # 编码过程中的临时计划（不入库）
├── scripts/                 # 辅助脚本（mock_llm_server.py）
├── rebuild_restart_app.sh   # 一键重编译
├── CLAUDE.md                # Agent 入口说明 + 海量教训
└── AGENTS.md                # 工程入口说明（= CLAUDE.md）
```

完整设计见 `docs/` 下各专题文档。

---

## 🧪 自动化测试 —— Agent 自检终端

项目使用两层测试：

| 层 | 入口 | 用途 |
|----|------|------|
| 单元测试 | `cargo test` | Rust 函数级覆盖（模块、解析、转换、工具） |
| 端到端(CLI) | `bash testReport/run_e2e.sh` | mock LLM，跑 `-p` / `provider` / 协议 wire / 项目上下文注入 / TUI 子屏 tmux 自动化 |

### TUI 子屏自动化（tmux control-mode）

```bash
# 起后台会话并启动 TUI，固定 100x30
tmux new-session -d -s laew_e2e -x 100 -y 30 ./laew
# 发送按键
tmux send-keys -t laew_e2e -l "/provider list"
tmux send-keys -t laew_e2e Enter
# 抓取面板断言
tmux capture-pane -p -t laew_e2e | grep -F "/provider list"
# 收尾
tmux kill-session -t laew_e2e
```

> 详见 `docs/TUI自动化测试/`。

---

## 📚 14 轮深度调研 —— Agent 编程的知识库

本仓库的 `docs/` 下沉淀了 **14 轮深度调研**，覆盖 **82+ 维度**，累计 **635 个 laew gap**（L1–L635），
全部由 Agent 自动产出：

| 轮次 | 主题 | 规模 |
|------|------|------|
| 第 1–6 轮 | 架构 / 多轮对话 / Context / 工具 / 记忆 / Workflow / Yolo / 质检 / MCP / Skill / 协议 wire / SubAgent / Goal / TUI / Hook | ~160k 行 |
| 第 7 轮 | 文件编辑 / 代码检索 / Git / Bash / 多模态 / PromptCaching / Schema / WebFetch | ~10k 行 |
| 第 8 轮 | Telemetry / Session / Tool 权限 / LSP / Hook / Skill / 多租户 / TUI | ~13.5k 行 |
| 第 9 轮 | CrashDump / WebUI / OAuth / i18n / Release / WebSocket / 容器 / CRDT | ~9.4k 行 |
| 第 10 轮 | 15 主文档追加新章节 | ~27k 行 |
| 第 11 轮 | Agent 协作 / 流式输出 / 错误处理 / 测试体系 / 配置系统 / 插件生态 / 协议翻译 / 系统提示词 | ~30k 行 |
| 第 12 轮 | HTTP 客户端 / 安全防御 / 模型路由 / 数据迁移 / 性能优化 / 日志 / CLI / 状态持久化 | ~17k 行 |
| 第 13 轮 | 本地推理 / KV cache / GUI 自动化 / 操作系统 / 评测基准 / 范式对比 / DSL / WebAssembly | ~14.9k 行 |
| 第 14 轮 | 8 大新维度 + 200 个新 gap | ~9.4k 行 |

> 专题合集见 `docs/专题/专题-第十三轮深挖合集.md` 等。
> 实现进度中央账本见 `docs/专题/专题-laew实现进度对照表.md`。

---

## 🤝 关注与支持

如果你觉得这个项目有趣，欢迎关注我在各平台的账号，观看完整的开发与演示视频：

| 平台 | 搜索账号 |
|------|---------|
| 快手 | **封刀灌海** |
| 抖音 | **封刀灌海** |
| B站 | **封刀灌海** |
| 小红书 | **封刀灌海** |
| 微信视频号 | **封刀灌海** |

---

## ☕ 打赏支持

项目的服务器、LLM API 调用、知识库沉淀等均有持续成本。如果这个项目对你有帮助或你觉得有趣，欢迎打赏支持：

| 微信打赏 | 支付宝打赏 |
|:--------:|:----------:|
| ![微信收款码](ProjectPic/微信二维码.jpg) | ![支付宝收款码](ProjectPic/支付宝二维码.jpg) |

> `ProjectPic/` 已随仓库入库，GitHub / Gitee / GitCode 上可直接查看收款码。

**联系方式**：

- 📱 手机：`13520647302`
- 💬 微信：`liushimeng109117198`

---

## 📜 协议

本项目以 **MIT License** 开放源代码 —— 详见 [`LICENSE`](LICENSE) 文件。

> Copyright (c) 2026 LsmAgentEmergentWork Authors
>
> 特此授予任何人免费获得本软件及相关文档文件（「软件」）副本的许可，
> 允许任何人无限制地处理本软件，包括但不限于使用、复制、修改、合并、发布、分发、
> 再许可和/或销售软件副本，并允许向其提供软件的人这样做，但须满足以下条件：
>
> 上述版权声明和本许可声明应包含在本软件的所有副本或实质性部分中。
>
> 本软件按「原样」提供，不提供任何形式的明示或默示保证，包括但不限于对适销性、
> 特定用途适用性和非侵权性的保证。在任何情况下，作者或版权持有人均不对任何索赔、
> 损害或其他责任负责，无论是合同行为、侵权行为还是其他原因，均与软件或软件的使用
> 或其他交易有关。

所有代码由 AI Agent 自动编写，人工 review 后入库。

---

## 🌟 Star / Watch / Fork

如果这个项目让你对「Agent 编程」有了新的认识，请：

- ⭐ **Star** 本仓库 —— 让更多人看到 Agent 编程的力量
- 👁️ **Watch** —— 跟进后续迭代
- 🍴 **Fork** —— 在你的环境里再造一座多 Agent CLI 平台

本仓库在双平台同步托管：

| 平台 | 链接 |
|------|------|
| Gitee | `https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork` |
| GitCode | `https://gitcode.com/liusm109117198/LsmAgentEmergentWork` |

> 💡 **如果 14 轮深度调研 / 635 个 gap / 100% Agent 自动编程的思路对你有启发**，欢迎在
> [Issues](https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork/issues) 分享你团队里的类似实践。
> 一个 ⭐ 比十篇博客更能推动这件事被更多人看见。

**这不是一个人写的代码。这是一群 AI Agent 24 小时不间断编程的作品。**

---

**版本**：v0.1.0  |  **最后更新**：2026-09-08  |  **构建**：Agent 自动构建
