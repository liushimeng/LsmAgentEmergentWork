//! LsmAgentEmergentWork - 基于 Rust 的 LLM Agent CLI 工程(`laew`)
//!
//! 模块划分:
//! - [`agent`]: Agent 核心循环(规划 -> 工具调用 -> 观察 -> 回答),含 system_prompt 与 tools 子模块
//! - [`llm`]:   大模型客户端抽象与 Anthropic/OpenAI 双协议实现
//! - [`config`]: 根目录/工作目录解析 + SQLite 接入记录存储
//! - [`tui`]:   交互式 REPL
//! - [`error`]: 统一错误类型

pub mod agent;
pub mod config;
pub mod error;
pub mod llm;
pub mod session;
pub mod tui;
