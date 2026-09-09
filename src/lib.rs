//! LsmAgentEmergentWork - 基于 Rust 的 LLM Agent CLI 工程(`laew`)
//!
//! 模块划分:
//! - [`agent`]: Agent 核心循环(规划 -> 工具调用 -> 观察 -> 回答),含 system_prompt 与 tools 子模块
//! - [`llm`]:   大模型客户端抽象与 Anthropic/OpenAI 双协议实现
//! - [`config`]: 配置层兼容模块(重导出 database 的核心类型,保留 session/agent memory)
//! - [`database`]: SQLite 数据库操作(路径解析 / Schema / Provider CRUD / 导入导出)
//! - [`tui`]:   交互式 REPL
//! - [`error`]: 统一错误类型

pub mod agent;
pub mod config;
pub mod crash;
pub mod database;
pub mod error;
pub mod llm;
pub mod session;
pub mod tui;
