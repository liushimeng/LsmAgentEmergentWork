//! 配置层
//!
//! 数据库相关代码已迁移到 [`crate::database`] 模块。
//! 本模块保留为兼容层，重导出核心类型，并保留 session_memory / agent_memory 子模块。

// 重导出 database 模块的公共类型，保持向后兼容
pub use crate::database::{
    format_context_size, parse_context_size, ConfigError, Db, ExportData, ExportRecord,
    ImportInput, ImportResult, Paths, Protocol, ProviderImport, ProviderRecord, Result,
    DEFAULT_CONTEXT_MAX_SIZE,
};

pub mod agent_memory;
pub mod session_memory;

// 重新导出子模块的类型
pub use agent_memory::{AgentMemoryEntry, AgentMemoryRow};
pub use session_memory::{EventType, SessionMemoryEntry, SessionMemoryRow};
