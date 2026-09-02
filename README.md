# LsmAgentEmergentWork

基于 Rust 的 Agent 工程。提供「LLM 规划 → 工具调用 → 观察 → 回答」的 Agent 循环骨架。

## 目录结构

```
src/
├── main.rs        # CLI 入口 (clap)
├── lib.rs         # 库导出
├── error.rs       # 统一错误类型 (thiserror)
├── agent/mod.rs   # Agent 核心循环
├── llm/mod.rs     # LLM 客户端 trait + OpenAI 兼容实现
└── tool/mod.rs    # Tool trait、ToolRegistry、内置工具
```

## 构建与运行

```bash
cargo build
cargo test

# 运行(兼容 OpenAI / DeepSeek / Qwen / Moonshot 等 OpenAI 协议接口)
export LLM_API_KEY=sk-xxx
export LLM_BASE_URL=https://api.deepseek.com   # 可选, 默认 https://api.openai.com
export LLM_MODEL=deepseek-chat                 # 可选

cargo run -- "现在是什么时间? 可用工具查询。"
```

## 扩展

- **新增工具**: 实现 `tool::Tool` trait, 注册进 `ToolRegistry`。
- **更换模型供应商**: 实现 `llm::LlmClient` trait, 或直接复用 `OpenAiClient`(OpenAI 兼容协议)。
- **记忆/检索**: 在 `agent::Agent::run` 的 `messages` 构造处接入向量检索模块。
