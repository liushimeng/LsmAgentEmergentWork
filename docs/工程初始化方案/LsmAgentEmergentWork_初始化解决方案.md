# LsmAgentEmergentWork 工程初始化解决方案（从 0 到 1）

> 版本：v0.1.0 ・ 日期：2026-09-02 ・ 状态：**已实现并验证**（单元测试 32/32，e2e 18/18，见 `testReport/验证报告-2026-09-02.md`）

## 1. 背景与目标

`LsmAgentEmergentWork` 是一个由 LLM 大语言模型驱动的 **CLI Agent 程序**（二进制名 `laew`），
目标能力包括 Work、Code、Design、ComputerUse、读写各种文件、操作各种软件/硬件等。
本阶段（第一阶段）完成 **基础框架**：

- 支持 **Anthropic 协议（anthropic-messages）** 与 **OpenAI 协议（openai-completions）** 双协议接入；
- 内置 **3 个工具调用**：`Bash`、`Read`、`Write`（含完整工具定义，参考 `docs/其他Agent工具定义/*`）；
- TUI 交互界面（多轮对话）+ `-p` 单轮（一次性任务）模式；
- 大模型接入记录（`provider_name` / `model_name` / `end_point` / `api_key` 四要素 + 协议类型）
  通过 CLI 配置，持久化到 **SQLite**（`根目录/LsmAgentEmergentWork.db`），不使用配置文件；
- `laew --version` 输出版本号与编译时间，`laew --help` 输出完整使用指南；
- `rebuild.sh` 一键杀进程、重编译、输出 `laew` 到工程根目录。

## 2. 关键概念与约束

| 概念 | 定义 |
|---|---|
| **根目录** | `laew` 二进制文件所在的目录（`std::env::current_exe()` 的父目录）。SQLite 数据库、编译产物均落在这里。 |
| **工作目录** | 启动 `laew` 命令时所在的目录（`std::env::current_dir()`），是 Agent 读写文件、执行命令的默认上下文。两者可能不同。 |
| **完整的大模型接入记录** | `protocol`（anthropic/openai）+ `provider_name`（平台名称）+ `model_name`（模型名称）+ `end_point`（接入点地址）+ `api_key` 五元组，可配置多条，任意时刻仅一条为「当前使用（is_active）」。 |
| **接入点自动补全** | Anthropic：`{end_point}/v1/messages`；OpenAI：`{end_point}/chat/completions`。`end_point` 末尾的 `/` 自动裁剪。 |

参考资料：

- `docs/协议抓包/*`：Claude Code（Anthropic Messages）、opencode（OpenAI Completions）等真实
  RequestBody/ResponseBody。其中 codex 走 responses 接口，仅参考其请求；其余作为主要参考。
- `docs/其他Agent工具定义/*`：claude-code、codex、hermes、openclaw、open-code、pi、WorkBuddy 等
  Agent 的工具定义（名称、描述、input_schema），用于编写本程序 `Bash`/`Read`/`Write` 的工具描述与参数 Schema。

## 3. 总体架构

```
┌────────────────────────────────────────────────────────────┐
│ main.rs (clap CLI)                                         │
│   laew            → tui::run()        交互式 TUI(多轮对话)  │
│   laew -p "..."   → 一次性任务模式                          │
│   laew provider * → 接入记录管理(增/删/列/切换)             │
│   --version / --help                                       │
├────────────────────────────────────────────────────────────┤
│ agent   协议无关的 Agent 循环：                             │
│         LLM 补全 → 有 tool_calls? → 执行工具 → 回填观察     │
│         → 再次补全 … 直至纯文本回答或达到最大迭代           │
├───────────────────┬────────────────────────────────────────┤
│ llm               │ tool                                   │
│ 统一消息模型       │ Tool trait + ToolRegistry              │
│ (ChatMessage/     │ 内置: Bash / Read / Write              │
│  ContentBlock/    │ 工具定义(name/description/input_schema) │
│  ToolDef)         │ 同时渲染为 Anthropic tools 与           │
│ LlmClient trait   │ OpenAI functions(tools) 两种 wire 格式  │
│ ├ AnthropicClient │                                        │
│ └ OpenAiClient    │                                        │
├───────────────────┴────────────────────────────────────────┤
│ config  根目录/工作目录解析 + SQLite(rusqlite bundled)      │
│         providers 表 CRUD / is_active 切换                  │
└────────────────────────────────────────────────────────────┘
```

### 3.1 目录结构

```
LsmAgentEmergentWork/
├── Cargo.toml / build.rs / rebuild.sh
├── laew                      # 编译产物（rebuild.sh 拷贝到根目录，gitignore）
├── LsmAgentEmergentWork.db   # SQLite 配置库（运行时生成，gitignore）
├── src/
│   ├── main.rs               # CLI 入口(clap)
│   ├── lib.rs                # 库根
│   ├── error.rs              # 统一错误类型
│   ├── config/mod.rs         # 路径解析 + SQLite 存储
│   ├── llm/
│   │   ├── mod.rs            # 统一消息模型 + LlmClient trait + 协议枚举
│   │   ├── anthropic.rs      # Anthropic Messages 客户端
│   │   └── openai.rs         # OpenAI Completions 客户端
│   ├── tool/
│   │   ├── mod.rs            # Tool trait / ToolRegistry
│   │   ├── bash.rs / read.rs / write.rs
│   ├── agent/mod.rs          # Agent 循环
│   └── tui/mod.rs            # rustyline 交互界面 + 斜杠命令
├── docs/                     # 知识库
├── tmpPlan/                  # 临时计划（gitignore）
└── testReport/               # 自动化测试报告
```

## 4. 技术设计

### 4.1 统一消息模型（协议无关核心）

协议差异在 `llm` 层内部消化，Agent 循环只面对统一模型：

```rust
pub enum Role { System, User, Assistant, Tool }

pub enum ContentBlock {
    Text(String),
    ToolUse  { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
}

pub struct ChatMessage { pub role: Role, pub content: Vec<ContentBlock> }

pub struct ToolDef { pub name: String, pub description: String,
                     pub input_schema: serde_json::Value }

pub struct Completion { pub text: String, pub tool_calls: Vec<ToolCallReq> }
pub struct ToolCallReq { pub id: String, pub name: String,
                         pub arguments: serde_json::Value }

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, system: &str, messages: &[ChatMessage],
                      tools: &[ToolDef]) -> Result<Completion>;
}
```

### 4.2 Anthropic 协议映射（`llm/anthropic.rs`）

- 请求：`POST {end_point}/v1/messages`；Header：`x-api-key`、`anthropic-version: 2023-06-01`、`content-type: application/json`。
- Body：`{model, max_tokens: 8192, system, messages, tools}`。
  - `system` 为顶层字段（不在 messages 里）；
  - 消息 content 为 block 数组：`text` / `tool_use{id,name,input}` / `tool_result{tool_use_id,content,is_error}`；
  - `tools: [{name, description, input_schema}]`。
- 响应：`content[]` 中 `text` 拼接为文本、`tool_use` 转为 `ToolCallReq`；`stop_reason` 仅记录日志。

### 4.3 OpenAI 协议映射（`llm/openai.rs`）

- 请求：`POST {end_point}/chat/completions`；Header：`Authorization: Bearer <api_key>`。
- Body：`{model, messages, tools, tool_choice: "auto"}`。
  - `system` 转为 `role: "system"` 消息；
  - assistant 的 ToolUse 转为 `tool_calls: [{id, type:"function", function:{name, arguments: JSON字符串}}]`；
  - ToolResult 转为 `role:"tool", tool_call_id, content`；
  - `tools: [{type:"function", function:{name, description, parameters}}]`（即 functions 风格的现代 tools 包装，与抓包一致）。
- 响应：`choices[0].message` 的 `content` 与 `tool_calls`（arguments 为 JSON 字符串，解析失败回退为 `{}` 并把原文并入 `__raw`）。

### 4.4 SQLite 存储设计（`config/mod.rs`）

```sql
CREATE TABLE IF NOT EXISTS providers (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  protocol      TEXT NOT NULL CHECK(protocol IN ('anthropic','openai')),
  provider_name TEXT NOT NULL,
  model_name    TEXT NOT NULL,
  end_point     TEXT NOT NULL,
  api_key       TEXT NOT NULL,
  is_active     INTEGER NOT NULL DEFAULT 0,
  created_at    TEXT NOT NULL DEFAULT (datetime('now','localtime')),
  UNIQUE(protocol, provider_name, model_name, end_point)
);
```

- 首次启动自动建库建表；`set_active(id)` 在事务中先清零再置一；
- `add` 时若库内无任何记录，则自动设为当前使用；
- DB 路径 = `根目录/LsmAgentEmergentWork.db`；根目录不可写时给出明确报错。

### 4.5 内置工具（`tool/`）

工具描述与 Schema 参考 `docs/其他Agent工具定义/claude-code/CC_{Bash,Read,Write}.md` 精简改写：

| 工具 | 参数 | 行为 |
|---|---|---|
| `Bash` | `command`(必填), `timeout_ms`(默认120s，最大600s), `description` | `bash -c` 执行，工作目录=工作目录，捕获 stdout/stderr/退出码，输出截断（默认 30000 字符） |
| `Read` | `file_path`(必填，绝对路径), `offset`, `limit` | 带行号读取（`cat -n` 风格），默认最多 2000 行，单行超长截断 |
| `Write` | `file_path`(必填，绝对路径), `content`(必填) | 覆盖写入/新建，自动创建父目录 |

### 4.6 Agent 循环（`agent/mod.rs`）

```
messages = [user(task)]
loop (≤ max_iterations, 默认 16):
    completion = llm.complete(system, messages, tools)
    if completion.tool_calls 为空 → 返回 completion.text
    messages += assistant(tool_use blocks + text)
    for call in tool_calls:
        output = registry.execute(call)   // 失败也转为 ToolResult(is_error=true)，不中断
        messages += user(tool_result blocks)
达到上限 → 返回 MaxIterationsExceeded 错误
```

多轮对话 = TUI 层持有同一条 `Vec<ChatMessage>` 历史跨轮复用。

### 4.7 CLI 与 TUI（`main.rs` / `tui/mod.rs`）

```
laew                                  # 进入 TUI 交互模式(多轮对话)
laew -p "提示词"                       # 一次性任务模式(单轮)
laew provider add --protocol anthropic --provider-name xxx \
                  --model-name xxx --end-point https://... --api-key sk-...
laew provider list                    # 列出全部接入记录(* 标记当前)
laew provider use <id>                # 切换当前记录
laew provider delete <id>             # 删除记录
laew --version                        # 版本号 + 编译时间 + git hash
laew --help                           # 完整使用指南
```

TUI（rustyline 行编辑 REPL，轻量 TUI）：

- 启动横幅展示：**根目录 / 工作目录 / 当前 provider_name / model_name / protocol**；
- 斜杠命令：`/help`、`/provider`（交互式增删列切换）、`/clear`（清空对话历史）、
  `/exit`（退出）；其余输入作为提示词进入多轮对话；
- 无接入记录时给出引导提示（先 `provider add`）。

### 4.8 构建与版本（`build.rs` / `rebuild.sh`）

- `build.rs` 注入 `LAEW_BUILD_TIME`（本地时区编译时间）与 `LAEW_GIT_HASH`；
- `rebuild.sh`：`pkill -x laew` → `cargo build --release` → 拷贝 `target/release/laew` 到工程根目录；
- `.gitignore` 增加 `/laew`、`*.db`、`tmpPlan/`。

## 5. 阶段划分与任务分解

| 阶段 | 内容 | 验收标准 | 状态 |
|---|---|---|---|
| P0 方案 | 本文档 + tmpPlan 临时计划 | 文档落盘 | ✅ |
| P1 构建骨架 | Cargo.toml（bin=laew、rusqlite、rustyline）、build.rs | `cargo fetch` 成功 | ✅（crates.io 慢，已配 rsproxy.cn 镜像） |
| P2 配置层 | config 模块 + SQLite CRUD + 单测 | 单测通过 | ✅ 6/6 |
| P3 协议层 | 统一消息模型 + 双客户端 + 序列化单测 | 单测通过 | ✅ 16/16 |
| P4 工具层 | Bash/Read/Write + 注册表 + 单测 | 单测通过 | ✅ 9/9 + 注册表 1/1 |
| P5 编排层 | Agent 循环 + main CLI + TUI | 编译通过 | ✅ |
| P6 构建验证 | rebuild.sh 跑通、修 bug、cargo test | 全绿 | ✅ 32/32 |
| P7 报告与提交 | testReport 输出、AGENTS.md/CLAUDE.md/README 更新、git 提交 | 提交完成 | ✅（e2e 18/18，`testReport/run_e2e.sh` 可复现） |

## 6. 风险与对策

1. **无真实 LLM 可用** → 协议正确性以「请求体序列化单测 + 抓包比对」保证；联网联调留作后续。
2. **SQLite 编译耗时** → `rusqlite/bundled` 首次编译较慢，一次性成本。
3. **reqwest TLS** → 使用 `rustls-tls` 避免系统 OpenSSL 依赖。
4. **TUI 复杂度** → 一期采用 rustyline 行式 TUI（满足横幅展示 + 多轮对话 + 斜杠命令），
   ratatui 全屏界面作为后续迭代。
5. **Bash 工具安全性** → 由模型自主调用，一期不加权限确认层（后续加确认/白名单），
   在系统提示词中约束使用。
