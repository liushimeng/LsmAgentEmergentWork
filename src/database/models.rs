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
    /// 上下文最大 Token 数(默认 800K;0 = 不限制,关闭自动压缩)
    pub context_max_size: u64,
}

/// ContextMaxSize 默认值:800K tokens。
pub const DEFAULT_CONTEXT_MAX_SIZE: u64 = 800_000;

/// 解析 ContextMaxSize 文本:支持纯数字(`800000`)与 K/M 后缀(`800K` / `1M`,大小写不敏感)。
pub fn parse_context_size(s: &str) -> std::result::Result<u64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("context_max_size 不能为空".to_string());
    }
    let (digits, mult) = match t.as_bytes().last() {
        Some(b'k') | Some(b'K') => (&t[..t.len() - 1], 1_000u64),
        Some(b'm') | Some(b'M') => (&t[..t.len() - 1], 1_000_000u64),
        _ => (t, 1u64),
    };
    let n: u64 = digits
        .trim()
        .parse()
        .map_err(|_| format!("无效的 context_max_size: '{s}'(支持 800000 / 800K / 1M,0 表示不限制)"))?;
    Ok(n.saturating_mul(mult))
}

/// 人类可读的大小显示(800000 → "800K")。
pub fn format_context_size(n: u64) -> String {
    if n == 0 {
        return "不限".to_string();
    }
    if n % 1_000_000 == 0 {
        format!("{}M", n / 1_000_000)
    } else if n % 1_000 == 0 {
        format!("{}K", n / 1_000)
    } else {
        n.to_string()
    }
}

// ===== 导入/导出相关结构体 =====

/// 导入用：单条 Provider 配置
///
/// `is_active` 可选：仅导出信封格式(`{"providers":[...]}`)的记录携带，
/// 用于导入后恢复原激活记录；手写配置可省略。
/// 反序列化时忽略未知字段(如导出格式里的 id / created_at)。
#[derive(Debug, Deserialize, Clone)]
pub struct ProviderImport {
    pub protocol: String,
    pub provider_name: String,
    pub model_name: String,
    pub end_point: String,
    pub api_key: String,
    #[serde(default)]
    pub is_active: Option<bool>,
    /// 可选:旧版(1.0)导出文件/手写配置无此字段,导入时补默认值 800K。
    #[serde(default)]
    pub context_max_size: Option<u64>,
}

/// 导入用：导出信封格式(`--outprovider` 产出的 JSON),支持原样再导入(往返兼容)
#[derive(Debug, Deserialize)]
pub struct ImportEnvelope {
    pub providers: Vec<ProviderImport>,
}

/// 导入输入：支持单条对象、对象数组或导出信封格式
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ImportInput {
    Batch(Vec<ProviderImport>),
    Envelope(ImportEnvelope),
    Single(ProviderImport),
}

impl ImportInput {
    /// 转换为统一的 Vec<ProviderImport>
    pub fn into_vec(self) -> Vec<ProviderImport> {
        match self {
            ImportInput::Single(one) => vec![one],
            ImportInput::Batch(vec) => vec,
            ImportInput::Envelope(env) => env.providers,
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
    pub context_max_size: u64,
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
                context_max_size: r.context_max_size,
            })
            .collect();

        Self {
            version: "1.1".to_string(),
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
