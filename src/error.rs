//! Agent 工程统一错误类型

use thiserror::Error;

use crate::config::ConfigError;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("配置错误: {0}")]
    Config(#[from] ConfigError),

    #[error("LLM 调用失败: {0}")]
    Llm(String),

    #[error("工具不存在: {0}")]
    ToolNotFound(String),

    #[error("工具执行失败[{tool}]: {reason}")]
    ToolExecution { tool: String, reason: String },

    #[error("达到最大迭代次数({0})仍未得到最终答案")]
    MaxIterationsExceeded(usize),

    #[error("Yolo 分类解析失败: {0}")]
    YoloParse(String),

    #[error("质量检查失败: {0} (源: {1})")]
    QualityFail(String, String),

    #[error("方案生成失败: {0}")]
    PlanGen(String),

    #[error("WorkFlow 解析失败: {0}")]
    WorkflowParse(String),

    #[error("WorkFlow 拓扑错误: {0}")]
    WorkflowTopology(String),

    #[error("编排失败: {0}")]
    Orchestration(String),

    #[error("HTTP 请求错误: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON 序列化错误: {0}")]
    Json(#[from] serde_json::Error),

    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),

    #[error("[沙箱拦截] 工具 {tool} 无法写入路径 \"{path}\"。\n允许范围:\n  - 工作目录: {work_dir}\n  - 系统临时目录: {temp_dir}\n请调整目标路径或使用允许目录内的路径。")]
    SandboxViolation {
        tool: String,
        path: String,
        work_dir: String,
        temp_dir: String,
    },
}

pub type Result<T> = std::result::Result<T, AgentError>;
