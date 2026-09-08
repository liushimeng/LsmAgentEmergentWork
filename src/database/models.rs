//! 数据模型定义
//!
//! 包含 Protocol 枚举、ProviderRecord 结构体，以及导入/导出专用的序列化结构体。

use serde::{Deserialize, Serialize};

/// LLM 接入协议
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Protocol {
    Anthropic,
    OpenAi,
}

impl Protocol {
    pub fn parse(s: &str) -> crate::database::Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "anthropic" => Ok(Protocol::Anthropic),
            "openai" => Ok(Protocol::OpenAi),
            other => Err(crate::database::ConfigError::InvalidProtocol(other.to_string())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Anthropic => "anthropic",
            Protocol::OpenAi => "openai",
        }
    }
}

/// 一条完整的大模型接入记录
#[derive(Debug, Clone)]
pub struct ProviderRecord {
    pub id: i64,
    pub protocol: Protocol,
    pub provider_name: String,
    pub model_name: String,
    pub end_point: String,
    pub api_key: String,
    pub is_active: bool,
    pub created_at: String,
}

// ===== 导入/导出相关结构体 =====

/// 导入用：单条 Provider 配置
#[derive(Debug, Deserialize, Clone)]
pub struct ProviderImport {
    pub protocol: String,
    pub provider_name: String,
    pub model_name: String,
    pub end_point: String,
    pub api_key: String,
}

/// 导入输入：支持单条对象或数组
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ImportInput {
    Single(ProviderImport),
    Batch(Vec<ProviderImport>),
}

impl ImportInput {
    /// 转换为统一的 Vec<ProviderImport>
    pub fn into_vec(self) -> Vec<ProviderImport> {
        match self {
            ImportInput::Single(one) => vec![one],
            ImportInput::Batch(vec) => vec,
        }
    }
}

/// 导出用：单条 Provider 记录（含 id 和 is_active）
#[derive(Debug, Serialize)]
pub struct ExportRecord {
    pub id: i64,
    pub protocol: String,
    pub provider_name: String,
    pub model_name: String,
    pub end_point: String,
    pub api_key: String,
    pub is_active: bool,
    pub created_at: String,
}

/// 导出用：完整导出数据结构
#[derive(Debug, Serialize)]
pub struct ExportData {
    pub version: String,
    pub exported_at: String,
    pub count: usize,
    pub providers: Vec<ExportRecord>,
}

impl ExportData {
    pub fn from_records(records: Vec<ProviderRecord>) -> Self {
        let count = records.len();
        let providers = records
            .into_iter()
            .map(|r| ExportRecord {
                id: r.id,
                protocol: r.protocol.as_str().to_string(),
                provider_name: r.provider_name,
                model_name: r.model_name,
                end_point: r.end_point,
                api_key: r.api_key,
                is_active: r.is_active,
                created_at: r.created_at,
            })
            .collect();

        Self {
            version: "1.0".to_string(),
            exported_at: time::OffsetDateTime::now_local()
                .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            count,
            providers,
        }
    }
}

/// 导入结果统计
#[derive(Debug, Default)]
pub struct ImportResult {
    pub success: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl ImportResult {
    pub fn total(&self) -> usize {
        self.success + self.skipped + self.failed
    }
}
