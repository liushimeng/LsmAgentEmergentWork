use thiserror::Error;

/// Agent 工程统一错误类型
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("LLM 调用失败: {0}")]
    Llm(String),

    #[error("工具不存在: {0}")]
    ToolNotFound(String),

    #[error("工具执行失败[{tool}]: {reason}")]
    ToolExecution { tool: String, reason: String },

    #[error("达到最大迭代次数({0})仍未得到最终答案")]
    MaxIterationsExceeded(usize),

    #[error("HTTP 请求错误: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON 序列化错误: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, AgentError>;
