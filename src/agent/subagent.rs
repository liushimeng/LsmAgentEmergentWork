//! SubAgent-Work Agent:执行层最小单元。
//!
//! 负责执行一个流程处理单元(subflow)的工作,持 Bash / Read / Write 全套工具。
//! 由 Orchestrator 调用,每次都是独立 Session 与上下文。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::cancel::CancelToken;
use crate::agent::context::AgentRole;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::memory;
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Usage};

/// SubFlow 输入(由 Orchestrator 构造)。
///
/// 2026-09-11 第三十三轮:新增 `original_prompt` 字段,让 SubAgent 在
/// `description`(Yolo 抽象摘要)之外仍能看到「用户原始 prompt」,
/// 解决 #P-A(任务漂移,具体任务被改写为「完成 laew 端到端链路验证」之类通用语)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubFlowInput {
    pub id: String,
    pub description: String,
    pub expected_output: String,
    /// 用户原始 prompt(Orchestrator 透传)。simple 任务必须填;
    /// medium/hard 由 MainWork/Plan 拆解后填各自的子任务原始输入。
    /// `None` 或空字符串视为未提供,`to_user_prompt()` 仅展示 description。
    #[serde(default)]
    pub original_prompt: Option<String>,
    /// 来自上游 WorkFlow 的产物(JSON 序列化字符串)
    #[serde(default)]
    pub depends_on_outputs: Vec<String>,
    /// 来自同一 WorkFlow 中前序步骤的产物
    #[serde(default)]
    pub sibling_outputs: Vec<String>,
}

impl SubFlowInput {
    pub fn to_user_prompt(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("【SubFlow id={}】\n", self.id));
        // 2026-09-11 第三十三轮:用户原始 prompt 优先于抽象摘要,
        // 避免 SubAgent 在脱离用户意图的「任务摘要」上空转。
        if let Some(orig) = self
            .original_prompt
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            out.push_str(&format!("用户原始输入:\n{}\n\n", orig));
        }
        out.push_str(&format!("任务摘要: {}\n", self.description));
        if !self.expected_output.is_empty() {
            out.push_str(&format!("期望输出: {}\n", self.expected_output));
        }
        if !self.depends_on_outputs.is_empty() {
            out.push_str("\n上游产物:\n");
            for (i, dep) in self.depends_on_outputs.iter().enumerate() {
                out.push_str(&format!("  - [依赖 {}] {}\n", i + 1, dep));
            }
        }
        if !self.sibling_outputs.is_empty() {
            out.push_str("\n同 WorkFlow 前序步骤产物:\n");
            for (i, s) in self.sibling_outputs.iter().enumerate() {
                out.push_str(&format!("  - [步骤 {}] {}\n", i + 1, s));
            }
        }
        out.push_str("\n请按期望输出完成任务。完成后用简洁中文回答(1-3 句话)。");
        out
    }
}

/// SubFlow 执行结果。
#[derive(Debug, Clone)]
pub struct SubFlowOutcome {
    pub text: String,
    pub usage: Usage,
    /// 是否判定为失败(多维判定:文本失败措辞 / 执行轨迹失败模式)
    pub failed: bool,
    /// 执行轨迹(QC 辅助判据 + Agent-Memory 持久化 + Yolo 失败回流)
    pub trace: ExecutionTrace,
}

/// SubAgent-Work 执行器。
pub struct SubAgentRunner {
    agent: Agent,
    db: Arc<Db>,
    max_iterations: usize,
}

impl SubAgentRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        let agent = Agent::new(llm, AgentProfile::sub_agent_work_profile());
        let max_iterations = agent.max_iterations();
        Self { agent, db, max_iterations }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self.agent = self.agent.with_max_iterations(n);
        self
    }

    /// 跑一次 SubFlow 单元。
    pub async fn run_unit(
        &self,
        input: &SubFlowInput,
        session_id: &str,
    ) -> Result<SubFlowOutcome> {
        self.run_unit_inner(input, session_id, None).await
    }

    /// [`run_unit`] 的可取消版本(第六轮 SubAgent 调度专题 §11.2 P0):
    /// 编排器把任务级取消 token 传入,Agent 循环内所有 LLM 调用 / 工具执行
    /// 即时中断并上抛 `AgentError::Cancelled`;取消路径不落 Agent-Memory。
    pub async fn run_unit_with_cancel(
        &self,
        input: &SubFlowInput,
        session_id: &str,
        cancel: &CancelToken,
    ) -> Result<SubFlowOutcome> {
        self.run_unit_inner(input, session_id, Some(cancel)).await
    }

    async fn run_unit_inner(
        &self,
        input: &SubFlowInput,
        session_id: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<SubFlowOutcome> {
        let prompt = input.to_user_prompt();
        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        // 让 sub_session 共享 session_id 便于追踪
        sub_session.id = session_id.to_string();

        // Agent 循环返回 (text, usage, trace) 三元组。
        // 早终止路径(RepeatedToolFailure / MaxIterationsExceeded)不再升级为 Error,
        // 而是包装成一段失败摘要文本 + 已填充 early_terminated 的 trace,
        // 让 Quality-Check 仍可基于 trace 判定 Fail。
        let (text, usage, mut trace) = match self
            .agent
            .run_session_cancellable(&mut sub_session, cancel)
            .await
        {
            Ok((t, u, tr)) => (t, u, tr),
            Err(AgentError::RepeatedToolFailure { tool, attempts, last_error }) => {
                let summary = format!(
                    "[RepeatedToolFailure] 工具 {tool} 连续 {attempts} 次失败;last_error: {last_error}"
                );
                let mut tr = ExecutionTrace::default();
                tr.early_terminated = true;
                tr.early_terminate_reason = format!("tool={tool} attempts={attempts}");
                tr.max_consecutive_failures = attempts;
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(AgentError::MaxIterationsExceeded(n)) => {
                let summary =
                    format!("[MaxIterationsExceeded] 迭代达到 {n} 次上限未得到最终答案");
                let mut tr = ExecutionTrace::default();
                tr.iterations = n;
                tr.early_terminated = true;
                tr.early_terminate_reason = format!("max_iter:{n}");
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(e) => return Err(e), // Cancelled / Llm 等真正错误依然上抛
        };

        // 计算多维失败信号 + 综合判定
        trace.collect_failure_signals(&text);
        let failed = trace.is_failed();

        // 写入 Agent-Memory(取消路径已在上面提前返回,不会走到这里)
        // 把 trace 关键指标塞进 artifacts,便于下次同类 Agent 加载经验。
        let error_summary_owned: Option<String> = if failed {
            if !trace.early_terminate_reason.is_empty() {
                Some(trace.early_terminate_reason.clone())
            } else {
                Some(trace.failure_signals.join(","))
            }
        } else {
            None
        };
        let error_summary = error_summary_owned.as_deref();
        let _ = memory::record_entry(
            &self.db,
            AgentRole::SubAgent,
            session_id,
            &input.description,
            &text,
            error_summary,
            serde_json::json!({
                "subflow_id": &input.id,
                "expected": &input.expected_output,
                "trace": &trace,
            }),
        );

        Ok(SubFlowOutcome { text, usage, failed, trace })
    }
}

/// 简单启发式:从 LLM 输出中探测是否包含失败标记。
///
/// 仅作辅助判定;真正的失败检测由 Quality-Check + ExecutionTrace 综合。
///
/// **2026-09-09 第 05 轮**:本函数仍保留以向后兼容既有测试,但
/// 实际判定已被 `ExecutionTrace::is_failed()` 取代。
///
/// **P0 增强**(2026-09-08):覆盖 LLM 中英双语常见失败措辞、大小写不敏感、
/// 容忍前缀空白,避免漏判 LLM 真实失败输出导致 silently pass。
/// 设计依据:`docs/Agent源码调研/专题-第八轮-Tool权限策略引擎与沙箱设计深度对比.md`
/// —— 5 态权限状态机 + fail-closed 默认值。
#[allow(dead_code)] // 单元测试仍覆盖该函数;运行时已被 ExecutionTrace::is_failed 取代
fn looks_like_failure(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let lower = t.to_lowercase();

    // 1) 英文失败关键词:大小写不敏感、容忍文本任意位置(LLM 自由格式输出)
    const EN_PATTERNS: &[&str] = &[
        "[failure]", "[failed]", "[error]", "failed:", "error:", "exception:",
        "fatal error", "panic:", "crash:", "aborted:", "killed:",
    ];
    if EN_PATTERNS.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // 2) 中文失败关键词:覆盖 LLM 中文输出的常见表达
    const ZH_PATTERNS: &[&str] = &[
        "[失败]", "执行失败", "未完成", "未能", "无法完成", "无法",
        "异常退出", "出错了",
    ];
    if ZH_PATTERNS.iter().any(|p| t.contains(p)) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use crate::llm::{ChatMessage, Completion, LlmClient, RequestMeta, Usage};

    /// 始终返回"rm -rf /" Bash 工具调用 → 触发 dangerous 命令拦截
    /// → 连续 3 次失败 → Agent 抛 RepeatedToolFailure 早终止。
    /// SubAgentRunner::run_unit 应捕获该错误,返回 Ok(outcome)
    /// 而非 Err(让 QC 仍可基于 trace 判定 Fail)。
    struct AlwaysBadBashLlm;

    #[async_trait]
    impl LlmClient for AlwaysBadBashLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &RequestMeta,
        ) -> crate::error::Result<Completion> {
            Ok(Completion {
                text: String::new(),
                tool_calls: vec![crate::llm::ToolCallReq {
                    id: "call-bad".into(),
                    name: "Bash".into(),
                    arguments: serde_json::json!({"command": "rm -rf /"}),
                }],
                usage: Usage::default(),
                stop_reason: None,
            })
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn run_unit_returns_outcome_on_repeated_tool_failure() {
        use crate::config::{Db, Paths};
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = std::sync::Arc::new(Db::open(&paths).unwrap());

        let llm: std::sync::Arc<dyn LlmClient> = std::sync::Arc::new(AlwaysBadBashLlm);
        let runner = SubAgentRunner::new(llm, db);

        let input = SubFlowInput {
            id: "wf-x".into(),
            description: "尝试运行 rm -rf /".into(),
            expected_output: "应该被拦截".into(),
            original_prompt: None,
            depends_on_outputs: vec![],
            sibling_outputs: vec![],
        };

        let outcome = runner.run_unit(&input, "s-x").await
            .expect("早终止应被 SubAgent 包装为 Ok(outcome),而非 Err");

        // 关键断言:outcome.trace.early_terminated=true
        assert!(outcome.trace.early_terminated,
                "RepeatedToolFailure 早终止应反映在 trace.early_terminated");
        assert!(!outcome.trace.early_terminate_reason.is_empty());
        assert!(outcome.trace.max_consecutive_failures >= 3);
        // outcome.failed=true(因 early_terminate 信号是强信号)
        assert!(outcome.failed,
                "outcome.failed 应为 true,触发 QC 判 Fail");
        // outcome.text 是早期包装的摘要
        assert!(outcome.text.contains("RepeatedToolFailure"),
                "outcome.text 应包含早终止摘要,实际: {}", outcome.text);
        // failure_signals 含 early_terminate
        assert!(outcome.trace.failure_signals.iter()
                .any(|s| s.starts_with("early_terminate:")),
                "failure_signals 应包含 early_terminate 信号");
    }

    #[test]
    fn subflow_input_to_user_prompt_contains_all_fields() {
        let input = SubFlowInput {
            id: "wf-1.step-1".into(),
            description: "读取 src/foo.rs".into(),
            expected_output: "返回文件前 50 行内容".into(),
            original_prompt: Some("请帮我看一下 src/foo.rs 这个文件的前 50 行".into()),
            depends_on_outputs: vec![],
            sibling_outputs: vec![],
        };
        let prompt = input.to_user_prompt();
        assert!(prompt.contains("wf-1.step-1"));
        assert!(prompt.contains("请帮我看一下 src/foo.rs 这个文件的前 50 行"));
        assert!(prompt.contains("读取 src/foo.rs"));
        assert!(prompt.contains("返回文件前 50 行内容"));
        // 原始 prompt 必须排在任务摘要之前,SubAgent 优先看到用户原始诉求
        let orig_pos = prompt.find("用户原始输入").unwrap();
        let summary_pos = prompt.find("任务摘要").unwrap();
        assert!(
            orig_pos < summary_pos,
            "original_prompt 必须在 description 之前呈现,避免 SubAgent 漂移到抽象摘要"
        );
    }

    #[test]
    fn subflow_input_with_deps() {
        let input = SubFlowInput {
            id: "wf-2".into(),
            description: "修改源文件".into(),
            expected_output: "替换函数 X".into(),
            original_prompt: None,
            depends_on_outputs: vec!["依赖产物 A".into()],
            sibling_outputs: vec!["前序步骤产物 B".into()],
        };
        let prompt = input.to_user_prompt();
        assert!(prompt.contains("上游产物"));
        assert!(prompt.contains("依赖产物 A"));
        assert!(prompt.contains("前序步骤产物 B"));
        // 未提供原始 prompt 时,不输出「用户原始输入」段
        assert!(!prompt.contains("用户原始输入"));
    }

    #[test]
    fn subflow_input_original_prompt_empty_treated_as_absent() {
        // 兼容性边界:Some("") 或纯空白字符串应等同 None,不污染 prompt。
        let input = SubFlowInput {
            id: "wf-3".into(),
            description: "测试".into(),
            expected_output: "".into(),
            original_prompt: Some("   \n  \t  ".into()),
            depends_on_outputs: vec![],
            sibling_outputs: vec![],
        };
        let prompt = input.to_user_prompt();
        assert!(!prompt.contains("用户原始输入"));
    }

    #[test]
    fn subflow_input_backward_compatible_default_field() {
        // 兼容性边界:不指定 original_prompt 时序列化与旧字段完全一致。
        let input = SubFlowInput {
            id: "wf-4".into(),
            description: "测试".into(),
            expected_output: "".into(),
            original_prompt: None,
            depends_on_outputs: vec![],
            sibling_outputs: vec![],
        };
        let json = serde_json::to_string(&input).unwrap();
        // original_prompt 默认值是 null,确保 SubAgent 输入 JSON 兼容老实现
        assert!(json.contains("\"original_prompt\":null"));
    }

    #[test]
    fn looks_like_failure_detects_prefix() {
        assert!(looks_like_failure("[失败] 原因: ..."));
        assert!(looks_like_failure("FAILED: ..."));
        assert!(looks_like_failure("ERROR: ..."));
        assert!(!looks_like_failure("已完成"));
        assert!(!looks_like_failure(""));
    }

    #[test]
    fn looks_like_failure_case_insensitive_english() {
        // 大小写不敏感(原本只命中大写 FAILED / ERROR)
        assert!(looks_like_failure("failed: timeout"));
        assert!(looks_like_failure("Failed: connection refused"));
        assert!(looks_like_failure("ERROR: panic"));
        assert!(looks_like_failure("Exception: NullPointerException"));
        assert!(looks_like_failure("Fatal Error: out of memory"));
        assert!(looks_like_failure("panic: thread main"));
        assert!(looks_like_failure("crash: segmentation fault"));
        assert!(looks_like_failure("aborted: signal 6"));
        assert!(looks_like_failure("killed: SIGTERM"));
    }

    #[test]
    fn looks_like_failure_chinese_variants() {
        // 中文常见失败措辞
        assert!(looks_like_failure("执行失败: 读取文件失败"));
        assert!(looks_like_failure("未完成: 编译报错"));
        assert!(looks_like_failure("未能找到目标文件"));
        assert!(looks_like_failure("无法读取 /etc/passwd"));
        assert!(looks_like_failure("无法完成该任务"));
        assert!(looks_like_failure("异常退出 code 1"));
        assert!(looks_like_failure("出错了: 网络超时"));
    }

    #[test]
    fn looks_like_failure_prefix_whitespace_tolerated() {
        // 容忍前缀空白 / 换行
        assert!(looks_like_failure("\n\n[失败] 原因: ..."));
        assert!(looks_like_failure("   FAILED: ..."));
    }

    #[test]
    fn looks_like_failure_negative_cases() {
        // 正向案例不应误判
        assert!(!looks_like_failure("task successfully completed"));
        assert!(!looks_like_failure("ok 已完成 30/30 用例"));
        assert!(!looks_like_failure("success"));
        assert!(!looks_like_failure("All tests passed"));
        assert!(!looks_like_failure("正常退出"));
    }

    #[test]
    fn looks_like_failure_no_substring_false_positive() {
        // 反例:不能因 "ok" 之类子串误判
        assert!(!looks_like_failure("ok"));
        assert!(!looks_like_failure("完成")); // "完成" 不在关键词列表
    }
}