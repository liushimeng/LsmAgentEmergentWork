# AGENTS.md — LsmAgentEmergentWork

供 AI Agent Tools（Claude Code / Codex / Hermes / OpenCode / pi / OpenClaw 等）自动加载的工程入口说明。

## 工程是什么

由 LLM 驱动的 Rust Agent CLI（二进制名 **`laew`**）。支持 Anthropic（anthropic-messages）与
OpenAI（openai-completions）双协议，内置 Bash / Read / Write 三个工具，TUI 多轮对话 + `-p` 单轮模式。
配置持久化在 **根目录** SQLite（`LsmAgentEmergentWork.db`），不使用配置文件。

## 常用命令

```bash
./rebuild.sh                 # 杀 laew 进程 → cargo build --release → 拷贝 ./laew 到根目录(改代码后必跑)
cargo build                  # 快速编译检查
cargo test                   # 单元测试(32 个)
bash testReport/run_e2e.sh   # 端到端(mock LLM,无需真实 Key,18 项)
./laew --version             # 版本 + 编译时间 + git hash
./laew --help                # CLI 指南
./laew                       # TUI 交互模式
./laew -p "任务描述"          # 单轮任务模式
./laew provider add|list|use|delete ...
```

注意：crates.io 在本机网络较慢，已在 `~/.cargo/config.toml` 配置 rsproxy.cn 镜像。

## 领域概念（改代码前必读）

- **根目录** = `laew` 二进制所在目录（`current_exe()` 父目录）。数据库 `LsmAgentEmergentWork.db`、编译产物 `./laew` 都在这里。
- **工作目录** = 启动命令时所在目录。Bash/Read/Write 工具的相对路径基准。两者可能不同，勿混淆。
- **接入记录（完整的大模型接入记录）** = `protocol(anthropic|openai) + provider_name + model_name + end_point + api_key` 五元组，存 SQLite `providers` 表，可多条，`is_active` 唯一。
- **接入点补全**：Anthropic → `{end_point}/v1/messages`；OpenAI → `{end_point}/chat/completions`；尾部 `/` 自动裁剪。
- **工具定义协议差异**：Anthropic 用 `tools[].{name,description,input_schema}`；OpenAI 用 `tools[].{type:"function",function:{name,description,parameters}}`（function 风格）。

## 架构（src/）

```
main.rs        clap CLI:默认进 tui; -p 单轮; provider 子命令
tui/mod.rs     rustyline REPL:横幅(根目录/工作目录/当前模型) + 斜杠命令(/provider /model /clear /exit)
agent/mod.rs   协议无关循环:complete → tool_calls → 执行 → tool_result 回填 → 直至纯文本
llm/mod.rs     统一消息模型 ChatMessage/ContentBlock/ToolDef + LlmClient trait(client_from_record 工厂)
llm/anthropic.rs  Anthropic wire 转换(x-api-key + anthropic-version)
llm/openai.rs      OpenAI wire 转换(Bearer)
tool/mod.rs    Tool trait + ToolRegistry(有序) + builtin_system_prompt
tool/bash|read|write.rs  三个内置工具
config/mod.rs  Paths::detect()(根/工作目录) + Db(SQLite CRUD)
error.rs       AgentError
build.rs       注入 LAEW_BUILD_TIME / LAEW_GIT_HASH(供 --version)
```

统一消息模型是关键设计：Agent 循环与工具层永远不接触协议细节，协议差异封闭在 `llm/*` 两个客户端内部。

## 文档地图（docs/）

- `docs/工程初始化方案/` — 从 0 到 1 的分阶段解决方案（架构 / 任务分解 / 技术设计）
- `docs/协议抓包/` — 各 Agent 真实 HTTP 抓包（RequestBody/ResponseBody）。**codex 走 responses 接口仅参考请求**，其余为主要参考
- `docs/其他Agent工具定义/` — claude-code / codex / hermes / openclaw / open-code / pi / WorkBuddy 等的工具定义，新增工具时先读这里

## 约定

- 注释、CLI 文案、文档一律中文；代码标识符英文。
- 新工具：在 `src/tool/` 建同名模块实现 `Tool` trait，注册进 `builtin_registry()`，Schema 参考 `docs/其他Agent工具定义/`。
- 新协议：实现 `LlmClient` trait + `client_from_record()` 增加分支，不改动 agent/tool 层。
- 测试报告输出到 `testReport/`（命名 `e2e-<时间戳>.txt` / `验证报告-<日期>.md`）；临时计划放 `tmpPlan/`（已 gitignore）。
- `laew`、`*.db`、`tmpPlan/`、`target/` 均不入库（见 .gitignore）。
