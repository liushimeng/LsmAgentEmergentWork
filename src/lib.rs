//! LsmAgentEmergentWork - 基于 Rust 的 Agent 工程
//!
//! 模块划分:
//! - [`agent`]: Agent 核心循环(规划 -> 工具调用 -> 观察 -> 回答)
//! - [`llm`]:   大模型客户端抽象与 OpenAI 兼容实现
//! - [`tool`]:  工具 trait、注册表与内置工具
//! - [`error`]: 统一错误类型

pub mod agent;
pub mod error;
pub mod llm;
pub mod tool;
