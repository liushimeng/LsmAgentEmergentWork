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

    /// Provider 熔断器打开:连续可重试失败达到阈值后的快速失败保护。
    ///
    /// 该错误本身不可重试,避免熔断器在同一个请求内被重试层放大;
    /// 冷却期到期后由 `ResilientLlmClient` 自动放入一个 HalfOpen 探测请求。
    #[error("LLM Provider 熔断器已打开:连续失败 {consecutive_failures} 次,约 {retry_in_ms}ms 后自动半开探测;请稍后重试或切换接入记录")]
    LlmCircuitOpen { retry_in_ms: u64, consecutive_failures: usize },

    #[error("工具不存在: {0}")]
    ToolNotFound(String),

    #[error("工具执行失败[{tool}]: {reason}")]
    ToolExecution { tool: String, reason: String },

    #[error("达到最大迭代次数({0})仍未得到最终答案")]
    MaxIterationsExceeded(usize),

    /// 用户中断(Ctrl-C / SIGINT)触发的任务取消。
    ///
    /// 语义约定:该错误**不可重试**——编排器遇到它必须短路退出整个任务,
    /// 不进入 QualityFailure / Yolo 失败回流;所有出口先补全 orphan tool_use
    /// 再上抛(协议一致性硬约束,anthropic/openai 均拒绝未配对的 tool_use)。
    /// 设计见 `tmpPlan/2026-09-08_07-取消传播与优雅中断方案.md`。
    #[error("任务已取消(用户中断)")]
    Cancelled,

    /// 连续相同「工具名 + 目标参数」失败次数超过阈值,主动提前终止以避免
    /// 浪费迭代预算(典型场景:上游 LLM 反复 Read 一个不存在的文件路径)。
    /// 关联报告: 20260908_203854 D-001。
    #[error("检测到连续 {attempts} 次相同工具 {tool} 调用失败,已提前终止 (last_error: {last_error})")]
    RepeatedToolFailure {
        tool: String,
        attempts: usize,
        last_error: String,
    },

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

    /// 工具参数 Schema 校验失败。
    ///
    /// 在工具执行前对 LLM 输出的 `tool_call.arguments` 做 JSON Schema 预校验,
    /// 校验失败时返回结构化错误信息,便于 LLM 理解并自我修复。
    /// 对应知识库 gap L16(第七轮结构化输出专题)。
    #[error("工具 {tool} 参数校验失败: 字段 '{path}' {reason}\n  期望: {expected}\n  实际: {actual}")]
    ToolSchemaValidation {
        tool: String,
        path: String,
        reason: String,
        expected: String,
        actual: String,
    },
}

pub type Result<T> = std::result::Result<T, AgentError>;
