//! Agent 工程统一错误类型

use thiserror::Error;

use crate::config::ConfigError;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("配置错误: {0}")]
    Config(#[from] ConfigError),

    #[error("LLM 调用失败: {0}")]
    Llm(String),

    /// 带 HTTP 状态码的 LLM 错误(结构化,供重试层判定可重试性)。
    /// `retry_after_ms` 来自 `Retry-After` 头(秒→毫秒),无则 None。
    #[error("LLM HTTP 错误(status={status}): {message}")]
    LlmHttp {
        status: u16,
        retry_after_ms: Option<u64>,
        message: String,
    },

    /// LLM 网络/超时类错误(连接失败、超时、SSE 传输中断、idle watchdog)。
    #[error("LLM 网络错误: {0}")]
    LlmNetwork(String),

    /// 上游在流内显式报错(SSE `error` 事件)。`kind` 为上游 error type
    /// (如 `overloaded_error` / `rate_limit_error`),供重试层分类。
    #[error("LLM 流式错误({kind}): {message}")]
    LlmStream { kind: String, message: String },

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

    #[error("[权限拒绝] 工具 {tool}: {reason}\n建议改用更安全的写法(参见工具描述中的「安全提示」)。")]
    PermissionDenied { tool: String, reason: String },
}

pub type Result<T> = std::result::Result<T, AgentError>;
