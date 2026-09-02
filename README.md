# LsmAgentEmergentWork

基于 Rust 的 LLM Agent CLI 工程（二进制名 `laew`）。支持 **Anthropic（anthropic-messages）**
与 **OpenAI（openai-completions）** 双协议接入，内置 `Bash` / `Read` / `Write` 三个工具调用，
提供 TUI 交互（多轮对话）与 `-p` 单轮（一次性任务）两种模式。

## 快速开始

```bash
./rebuild.sh        # 杀掉旧进程 → cargo build --release → 输出 ./laew
./laew --version    # 版本号 + 编译时间 + git hash
./laew --help       # 完整使用指南

# 配置一条大模型接入记录(protocol + provider_name + model_name + end_point + api_key)
./laew provider add --protocol anthropic \
    --provider-name myAnthropic --model-name claude-sonnet-5 \
    --end-point https://api.anthropic.com --api-key sk-ant-xxxx

./laew provider list          # 查看(* 为当前使用)
./laew provider use 2         # 切换当前模型
./laew provider delete 2      # 删除记录

./laew                        # 进入 TUI 多轮对话(横幅含根目录/工作目录/当前模型)
./laew -p "帮我看看当前目录有什么文件"   # 单轮任务模式
```

接入点自动补全：Anthropic 自动拼接 `v1/messages`，OpenAI 自动拼接 `chat/completions`。
配置不落配置文件，全部存于 **根目录** 的 SQLite（`LsmAgentEmergentWork.db`）。

## 关键概念

| 概念 | 说明 |
|---|---|
| 根目录 | `laew` 二进制所在目录；数据库与编译产物落在这里 |
| 工作目录 | 启动 `laew` 时所在目录；Agent 读写文件 / 执行命令的默认上下文 |
| 接入记录 | protocol + provider_name + model_name + end_point + api_key 五元组，可配多条，仅一条激活 |

## 目录结构

```
src/
├── main.rs          # CLI 入口 (clap): TUI / -p / provider 子命令
├── lib.rs           # 库导出
├── error.rs         # 统一错误类型 (thiserror)
├── config/mod.rs    # 根目录/工作目录解析 + SQLite(rusqlite) 接入记录存储
├── agent/mod.rs     # Agent 核心循环(协议无关)
├── llm/
│   ├── mod.rs       # 统一消息模型 + LlmClient trait
│   ├── anthropic.rs # Anthropic Messages 客户端
│   └── openai.rs    # OpenAI Chat Completions 客户端
├── tool/
│   ├── mod.rs       # Tool trait / ToolRegistry
│   ├── bash.rs      # Bash 工具
│   ├── read.rs      # Read 工具
│   └── write.rs     # Write 工具
└── tui/mod.rs       # rustyline 行式 TUI(REPL)

docs/      # 知识库:协议抓包、其他 Agent 工具定义、工程方案文档
testReport/# 自动化测试报告(cargo test + run_e2e.sh)
tmpPlan/   # 编码过程中的临时计划(不入库)
scripts/   # 辅助脚本(mock_llm_server.py)
rebuild.sh # 一键重编译
```

## 测试

```bash
cargo test                   # 32 个单元测试
bash testReport/run_e2e.sh   # 端到端:mock LLM 验证双协议工具调用循环(18 项)
```

详见 `docs/工程初始化方案/LsmAgentEmergentWork_初始化解决方案.md` 与 `testReport/验证报告-*.md`。
