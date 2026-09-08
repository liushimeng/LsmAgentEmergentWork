//! SubAgent-Work Agent:执行层最小单元。
//!
//! 负责执行一个流程处理单元(subflow)的工作,持 Bash / Read / Write 全套工具。
//! 由 Orchestrator 调用,每次都是独立 Session 与上下文。

use std::sync::Arc;

use serde::Serialize;

use crate::agent::context::AgentRole;
use crate::agent::memory;
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::Result;
use crate::llm::{ChatMessage, Usage};

/// SubFlow 输入(由 Orchestrator 构造)。
#[derive(Debug, Clone, Serialize)]
pub struct SubFlowInput {
    pub id: String,
    pub description: String,
    pub expected_output: String,
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
        out.push_str(&format!("任务: {}\n", self.description));
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
    /// 是否判定为失败(根据 LLM 输出推断)
    pub failed: bool,
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
        let prompt = input.to_user_prompt();
        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        // 让 sub_session 共享 session_id 便于追踪
        sub_session.id = session_id.to_string();

        let (text, usage) = self.agent.run_session(&mut sub_session).await?;

        // 写入 Agent-Memory
        let _ = memory::record_entry(
            &self.db,
            AgentRole::SubAgent,
            session_id,
            &input.description,
            &text,
            None,
            serde_json::json!({ "subflow_id": &input.id, "expected": &input.expected_output }),
        );

        let failed = looks_like_failure(&text);
        Ok(SubFlowOutcome { text, usage, failed })
    }
}

/// 简单启发式:从 LLM 输出中探测是否包含失败标记。
///
/// 仅作辅助判定;真正的失败检测由 Quality-Check 完成。
///
/// **P0 增强**(2026-09-08):覆盖 LLM 中英双语常见失败措辞、大小写不敏感、
/// 容忍前缀空白,避免漏判 LLM 真实失败输出导致 silently pass。
/// 设计依据:`docs/Agent源码调研/专题-第八轮-Tool权限策略引擎与沙箱设计深度对比.md`
/// —— 5 态权限状态机 + fail-closed 默认值。
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

    #[test]
    fn subflow_input_to_user_prompt_contains_all_fields() {
        let input = SubFlowInput {
            id: "wf-1.step-1".into(),
            description: "读取 src/foo.rs".into(),
            expected_output: "返回文件前 50 行内容".into(),
            depends_on_outputs: vec![],
            sibling_outputs: vec![],
        };
        let prompt = input.to_user_prompt();
        assert!(prompt.contains("wf-1.step-1"));
        assert!(prompt.contains("读取 src/foo.rs"));
        assert!(prompt.contains("返回文件前 50 行内容"));
    }

    #[test]
    fn subflow_input_with_deps() {
        let input = SubFlowInput {
            id: "wf-2".into(),
            description: "修改源文件".into(),
            expected_output: "替换函数 X".into(),
            depends_on_outputs: vec!["依赖产物 A".into()],
            sibling_outputs: vec!["前序步骤产物 B".into()],
        };
        let prompt = input.to_user_prompt();
        assert!(prompt.contains("上游产物"));
        assert!(prompt.contains("依赖产物 A"));
        assert!(prompt.contains("前序步骤产物 B"));
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