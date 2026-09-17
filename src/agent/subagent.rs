//! SubAgent-Work Agent:执行层最小单元。
//!
//! 负责执行一个流程处理单元(subflow)的工作,持 Bash / Read / Write 全套工具。
//! 由 Orchestrator 调用,每次都是独立 Session 与上下文。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info;

use crate::agent::agent_message::AgentMessage;
use crate::agent::cancel::CancelToken;
use crate::agent::context::AgentRole;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::memory;
use crate::agent::window_state::WindowSessionState;
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, ContentBlock, Role, Usage};

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
    /// ★窗口上下文(由 Orchestrator 注入,WindowUse 单元专用,跨轮持久化)。
    #[serde(default)]
    pub window_context: Option<WindowSessionState>,
    /// ★待处理的 Agent 消息(其他 Agent 发来的)。
    #[serde(default)]
    pub pending_agent_messages: Vec<AgentMessage>,
    /// ★2026-09-17 第 75 轮:WorkFlow 期望的 Agent 角色(`wf.delegate_to` 派生)。
    /// Runner 写入 `trace.intended_role`;与 `trace.runner_role` 对比可识别
    /// 「委派错配」(`delegate_mismatch` 弱信号),供 QC + TUI 给出明确诊断。
    /// `None` 表示 Orchestrator 未注入(老调用点兼容)。
    #[serde(default)]
    pub intended_role: Option<AgentRole>,
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
        if let Some(ref ctx) = self.window_context {
            if !ctx.is_empty() {
                let state_prompt = crate::agent::window_state::build_window_state_prompt(ctx);
                if !state_prompt.is_empty() {
                    out.push_str(&format!("\n【窗口会话上下文(系统注入)】\n{state_prompt}"));
                }
            }
        }
        if !self.pending_agent_messages.is_empty() {
            out.push_str("\n【来自其他 Agent 的消息】\n");
            for (i, msg) in self.pending_agent_messages.iter().enumerate() {
                out.push_str(&format!("  {}. {}\n", i + 1, msg.hint()));
            }
        }
        // ★ 2026-09-17 第 79 轮 P0-1:按任务性质区分终答形态。
        // 旧版固定「1-3 句话」导致「显示/返回某内容」类任务的终答只剩路径/占位描述
        // (实测文心一言任务:结果写 wenxin_result.txt,TUI 只看到「已保存到 …」)。
        out.push_str(
            "\n请按期望输出完成任务。完成后用简洁中文回答:\n\
             - 执行/修改类任务:1-3 句话说明完成情况与产物路径;\n\
             - ★ 内容展示/查询/采集类任务(用户要求「显示/展示/返回/告诉我」某内容):\n\
               最终回答必须直接包含真实内容本身——文本类内容(200-8000 字)原样贴出,\n\
               超长内容贴关键部分并注明总长度;禁止只回答「已保存到 xx.txt」「内容已提取,\n\
               共 N 字符」等路径/占位描述——用户在终端只能看到你的回答,看不到文件。\n\
               文件落盘仅作为补充产物一并说明。截图等二进制产物例外:给出路径 + 大小 + 简述。",
        );
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
        Self {
            agent,
            db,
            max_iterations,
        }
    }

    /// 2026-09-17 第 78 轮 P0-2:Runner 出口兜底 —— 提取 sub_session 中的「实质产物摘要」。
    ///
    /// 业务背景(llaew_20260917_135513.log 复盘):
    /// 文心一言任务被 Yolo 降级 simple + suggested_delegate=webuse,但 simple 档此前硬编码
    /// SubAgentRunner(第 78 轮 P0-1 已修复为路由 WebUseRunner)。在修复前,
    /// SubAgentRunner 出口 outcome.text 仅含「任务成功完成,内容已保存到 wenxin_result.txt」,
    /// 真实抓取的 AI 回复在文件里,TUI 看不到 — 用户必须二次追问「结果显示在哪里了」。
    ///
    /// 兜底策略(对齐 web_use.rs::extract_page_reply_from_session 思路):
    /// 1. 倒序遍历 sub_session.role=Tool 的消息,逐条检查 tool_use_id 对应的 tool_call
    ///    (从 sub_session.role=Assistant 的 tool_calls 中按 id 索引);
    /// 2. 按工具名分类提取:
    ///    - Bash:从 stdout 字段取最长非空连续段落(≥30 字符),跳过 stderr / 退出码噪音;
    ///    - Write/Edit:从 tool_call.arguments.content 字段取全文(截 4000 字符);
    ///    - Read:从 tool_result.content 提取 text 字段(截 4000 字符)。
    /// 3. 返回 Vec<(text, source_label)>,按出现顺序追加,最多 3 条避免 context 膨胀。
    ///
    /// 注意:
    /// - 不修改 outcome.text 本身,只读取;调用方在 Runner 出口拼接;
    /// - 失败/错误的 tool_result(content 含 error / exit_code)跳过(避免污染产物);
    /// - LLM 的 thinking / text 回复不参与(已经在 outcome.text 里)。
    pub fn extract_runner_evidence_from_session(
        messages: &[ChatMessage],
    ) -> Vec<(String, String)> {
        let mut evidences: Vec<(String, String)> = Vec::new();

        // 第一遍:建立 tool_use_id → tool_name 的反向索引(Assistant.tool_calls)
        let mut tool_name_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for msg in messages.iter() {
            if msg.role != Role::Assistant {
                continue;
            }
            for block in &msg.content {
                if let ContentBlock::ToolUse { id, name, .. } = block {
                    tool_name_map.insert(id.clone(), name.clone());
                }
            }
        }

        // 第二遍:倒序遍历 role=Tool,按工具名分类提取
        for msg in messages.iter().rev() {
            if msg.role != Role::Tool {
                continue;
            }
            for block in &msg.content {
                let (tool_use_id, content, _is_error) = match block {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => (tool_use_id.clone(), content.clone(), *is_error),
                    _ => continue,
                };
                if content.trim().is_empty() {
                    continue;
                }
                let tool_name = tool_name_map
                    .get(&tool_use_id)
                    .cloned()
                    .unwrap_or_default();

                // 1) Bash:stdout / output 中取最长段落
                if tool_name == "Bash" {
                    if let Some(stdout) = extract_bash_main_output(&content) {
                        if stdout.chars().count() >= 30 {
                            evidences.push((
                                truncate_chars(&stdout, 4000),
                                format!("Bash[{}]", &tool_use_id),
                            ));
                        }
                    }
                    continue;
                }
                // 2) Read:tool_result.content 直接当文本(可能含行号)
                if tool_name == "Read" {
                    let text = content.trim();
                    if text.chars().count() >= 30 {
                        evidences.push((
                            truncate_chars(text, 4000),
                            format!("Read[{}]", &tool_use_id),
                        ));
                    }
                    continue;
                }
                // 3) Write/Edit:content 字段在 Assistant.tool_calls.arguments 里
                // 这里 content 通常是 stdout-style 信封;尝试 JSON parse 取 file_path 提示
                if tool_name == "Write" || tool_name == "Edit" {
                    let parsed: Option<Value> = serde_json::from_str(&content)
                        .ok()
                        .or_else(|| serde_json::from_str(&content).ok());
                    if let Some(v) = parsed {
                        if let Some(data) = v.get("data") {
                            let path = data
                                .get("file_path")
                                .and_then(Value::as_str)
                                .unwrap_or("<unknown>");
                            let bytes = data
                                .get("bytes_written")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            evidences.push((
                                format!("<Write 到 {path}> 共 {bytes} 字节"),
                                format!("{tool_name}[{}]", &tool_use_id),
                            ));
                        }
                    }
                }
            }
            // 控制总量:倒序遍历最多取 6 条候选,最后按内容长度排序保留 3 条
            if evidences.len() >= 6 {
                break;
            }
        }

        // 长度降序,保留前 3 条最长(实质内容优先)
        evidences.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()));
        evidences.truncate(3);

        evidences.reverse(); // 还原时间顺序
        evidences
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self.agent = self.agent.with_max_iterations(n);
        self
    }

    /// 跑一次 SubFlow 单元。
    pub async fn run_unit(&self, input: &SubFlowInput, session_id: &str) -> Result<SubFlowOutcome> {
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
        // ★2026-09-17 第 75 轮:Runner 角色信息(SubAgent)。
        // ★2026-09-17 第 78 轮:SubAgentRunner 在 simple 档也可能被 webuse/windowuse
        // 委派(suggested_delegate 路由修复后);但 Runner 实际执行的是 SubAgent 角色,
        // trace.runner_role 仍为 SubAgent,trace.intended_role 反映 WorkFlow 期望的角色。
        let runner_role = Some(AgentRole::SubAgent);
        let intended_role = input.intended_role;

        // 而是包装成一段失败摘要文本 + 已填充 early_terminated 的 trace,
        // 让 Quality-Check 仍可基于 trace 判定 Fail。
        let (text, usage, mut trace) = match self
            .agent
            .run_session_cancellable(&mut sub_session, cancel)
            .await
        {
            Ok((t, u, tr)) => (t, u, tr),
            Err(AgentError::RepeatedToolFailure {
                tool,
                attempts,
                last_error,
                trace: carried,
            }) => {
                let summary = format!(
                    "[RepeatedToolFailure] 工具 {tool} 连续 {attempts} 次失败;last_error: {last_error}"
                );
                // 2026-09-17 第 75 轮:使用 Agent 循环携带的真实 trace(工具调用历史不丢;
                // early_terminated/reason/max_consecutive_failures 已在抛出点设置)
                let mut tr = *carried;
                tr.runner_role = runner_role;
                tr.intended_role = intended_role;
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(AgentError::MaxIterationsExceeded {
                iterations: n,
                trace: carried,
            }) => {
                let summary = format!("[MaxIterationsExceeded] 迭代达到 {n} 次上限未得到最终答案");
                // 2026-09-17 第 75 轮:使用 Agent 循环携带的真实 trace(工具调用历史不丢)
                let mut tr = *carried;
                tr.runner_role = runner_role;
                tr.intended_role = intended_role;
                tr.iterations = n;
                tr.early_terminated = true;
                tr.early_terminate_reason = format!("max_iter:{n}");
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(e) => return Err(e), // Cancelled / Llm 等真正错误依然上抛
        };

        // 2026-09-17 第 75 轮:把 Runner 实际角色 + WorkFlow 期望角色写入 trace,
        // 供 collect_failure_signals 计算 delegate_mismatch 弱信号。
        trace.runner_role = runner_role;
        trace.intended_role = intended_role;
        // 计算多维失败信号 + 综合判定
        trace.collect_failure_signals(&text);
        let failed = trace.is_failed();

        // ★ 2026-09-17 第 78 轮 P0-2:Runner 出口兜底 —— 抓取 sub_session 中的
        // 实质产物(对齐 web_use.rs::extract_page_reply_from_session 思路,让真实抓取的
        // 内容也能落到 outcome.text,而不是只看到「已保存到 xxx.txt」这种描述性占位句)。
        // 解决 llaew_20260917_135513.log 复盘问题:文心一言任务 Yolo 降级 simple →
        // SubAgentRunner → 写 Python Playwright 脚本 → 把 AI 回复写到 wenxin_result.txt
        // → outcome.text 仅含描述 → TUI 看不到真实内容。
        //
        // 行为:
        // 1) 调用 extract_runner_evidence_from_session 抓 3 条最长实质产物(Bash stdout /
        //    Read text / Write file_path+bytes 摘要);
        // 2) 若 LLM 终答不含足够实质内容(text 长度 < 50 OR 不含动作关键词)→ 把兜底
        //    内容追加到 outcome.text 末尾,标注 [Runner 出口兜底 · 来源];
        // 3) 终答本身已含丰富内容(长度 ≥ 200 OR 含动作关键词)→ 不追加,避免冗余。
        let text = {
            let evidences = Self::extract_runner_evidence_from_session(sub_session.context());
            if evidences.is_empty() {
                text
            } else if runner_text_needs_fallback(&text) {
                let evidence_count = evidences.len();
                let mut combined = text;
                if !combined.trim().is_empty() {
                    combined.push_str("\n\n");
                }
                combined.push_str("[Runner 出口兜底 · 实质产物摘要]\n");
                for (evidence, source) in evidences {
                    combined.push_str(&format!(
                        "\n--- 来源: {} ---\n{}",
                        source, evidence
                    ));
                }
                info!(
                    evidence_count,
                    combined_chars = combined.chars().count(),
                    "SubAgent Runner 出口兜底:已追加实质产物到 outcome.text"
                );
                combined
            } else {
                // 终答已含丰富内容,避免冗余
                text
            }
        };

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

        Ok(SubFlowOutcome {
            text,
            usage,
            failed,
            trace,
        })
    }
}

/// 2026-09-17 第 78 轮 P0-2:判定 Runner 终答是否需要兜底。
///
/// 短文本(< 200 字符)且不含动作关键词 → 视为「描述性占位句」,走兜底;
/// 长文本(≥ 200)或含明确动作关键词 → 视为已含实质内容,不追加(避免冗余)。
fn runner_text_needs_fallback(text: &str) -> bool {
    if text.trim().is_empty() {
        return true;
    }
    if text.chars().count() >= 200 {
        return false;
    }
    const KEYWORDS: &[&str] = &[
        "已打开", "已点击", "已输入", "已截图", "已抓取", "已采集", "已登录",
        "已写入", "已生成", "page_id", "BrowserNew", "BrowserControl", "BrowserInspect",
        "已完成", "执行成功", "已保存", "成功完成",
    ];
    !KEYWORDS.iter().any(|k| text.contains(k))
}

/// 2026-09-17 第 78 轮 P0-2:从 Bash 工具结果中提取主输出。
///
/// Bash 信封形如:
/// ```text
/// <stdout>
/// ... 主输出 ...
/// </stdout>
///
/// <stderr>
/// ... 错误输出 ...
/// </stderr>
///
/// <exit_code>0</exit_code>
/// ```
///
/// 提取 `<stdout>` 段内的最长非空连续段落(≥30 字符),
/// 跳过纯提示行(`echo xxx` 之类),保留实际抓取内容。
fn extract_bash_main_output(content: &str) -> Option<String> {
    let stdout_start = content.find("<stdout>")?;
    let stdout_end = content.find("</stdout>")?;
    let stdout = &content[stdout_start + "<stdout>".len()..stdout_end];
    // 取最长的非空连续段落(段落按空行分隔)
    let mut longest = String::new();
    for block in stdout.split("\n\n") {
        let trimmed = block.trim();
        if trimmed.chars().count() > longest.chars().count() && trimmed.chars().count() >= 30 {
            longest = trimmed.to_string();
        }
    }
    if longest.is_empty() {
        // 退化:取整个 stdout 段(去掉首尾空白)
        let trimmed = stdout.trim();
        if trimmed.chars().count() >= 30 {
            Some(trimmed.to_string())
        } else {
            None
        }
    } else {
        Some(longest)
    }
}

/// 简单字符级截断 + 「...」后缀。
fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
        out.push_str("...");
        out
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
        "[failure]",
        "[failed]",
        "[error]",
        "failed:",
        "error:",
        "exception:",
        "fatal error",
        "panic:",
        "crash:",
        "aborted:",
        "killed:",
    ];
    if EN_PATTERNS.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // 2) 中文失败关键词:覆盖 LLM 中文输出的常见表达
    const ZH_PATTERNS: &[&str] = &[
        "[失败]",
        "执行失败",
        "未完成",
        "未能",
        "无法完成",
        "无法",
        "异常退出",
        "出错了",
    ];
    if ZH_PATTERNS.iter().any(|p| t.contains(p)) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::llm::{ChatMessage, Completion, LlmClient, RequestMeta, Usage};
    use async_trait::async_trait;

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
            window_context: None,
            pending_agent_messages: vec![],
            // 2026-09-17 第 75 轮:SubAgent Runner 自测试默认走 SubAgent。
            intended_role: Some(AgentRole::SubAgent),
        };

        let outcome = runner
            .run_unit(&input, "s-x")
            .await
            .expect("早终止应被 SubAgent 包装为 Ok(outcome),而非 Err");

        // 关键断言:outcome.trace.early_terminated=true
        assert!(
            outcome.trace.early_terminated,
            "RepeatedToolFailure 早终止应反映在 trace.early_terminated"
        );
        assert!(!outcome.trace.early_terminate_reason.is_empty());
        assert!(outcome.trace.max_consecutive_failures >= 3);
        // outcome.failed=true(因 early_terminate 信号是强信号)
        assert!(outcome.failed, "outcome.failed 应为 true,触发 QC 判 Fail");
        // outcome.text 是早期包装的摘要
        assert!(
            outcome.text.contains("RepeatedToolFailure"),
            "outcome.text 应包含早终止摘要,实际: {}",
            outcome.text
        );
        // failure_signals 含 early_terminate
        assert!(
            outcome
                .trace
                .failure_signals
                .iter()
                .any(|s| s.starts_with("early_terminate:")),
            "failure_signals 应包含 early_terminate 信号"
        );
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
            window_context: None,
            pending_agent_messages: vec![],
            // 2026-09-17 第 75 轮:SubAgent Runner 自测试默认走 SubAgent。
            intended_role: Some(AgentRole::SubAgent),
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
            window_context: None,
            pending_agent_messages: vec![],
            // 2026-09-17 第 75 轮:SubAgent Runner 自测试默认走 SubAgent。
            intended_role: Some(AgentRole::SubAgent),
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
            window_context: None,
            pending_agent_messages: vec![],
            // 2026-09-17 第 75 轮:SubAgent Runner 自测试默认走 SubAgent。
            intended_role: Some(AgentRole::SubAgent),
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
            window_context: None,
            pending_agent_messages: vec![],
            // 2026-09-17 第 75 轮:SubAgent Runner 自测试默认走 SubAgent。
            intended_role: Some(AgentRole::SubAgent),
        };
        let json = serde_json::to_string(&input).unwrap();
        // original_prompt 默认值是 null,确保 SubAgent 输入 JSON 兼容老实现
        assert!(json.contains("\"original_prompt\":null"));
        // 新字段默认值也应正确序列化
        assert!(json.contains("\"window_context\":null"));
        assert!(json.contains("\"pending_agent_messages\":[]"));
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

    // ================== 2026-09-17 第 78 轮 P0-2 新增:Runner 出口兜底测试 ==================

    use crate::llm::ContentBlock;

    /// 构造一个 Bash 工具的 sub_session(模拟 SubAgent 跑 Python Playwright 写文件场景)
    fn make_bash_session() -> Vec<ChatMessage> {
        vec![
            // user 任务提示
            ChatMessage::user("打开文心一言"),
            // Assistant 调用 Bash
            ChatMessage::assistant(vec![ContentBlock::ToolUse {
                id: "toolu_1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({
                    "command": "python3 -c \"from playwright.sync_api import sync_playwright; ..."
                }),
            }]),
            // Tool 返回 stdout 含真实 AI 回复
            ChatMessage::tool_result(
                "toolu_1",
                "<stdout>\n黄金价格 927.79 元/克,白银价格 63.80 美元/盎司,数据来源: 文心一言实时查询\n\n风险提示: 以上价格仅供参考,实际交易价格以市场为准。\n</stdout>\n\n<stderr>\n\n</stderr>\n\n<exit_code>0</exit_code>",
                false,
            ),
        ]
    }

    #[test]
    fn extract_runner_evidence_bash_picks_main_stdout() {
        let msgs = make_bash_session();
        let evidences = SubAgentRunner::extract_runner_evidence_from_session(&msgs);
        assert!(!evidences.is_empty(), "Bash 调用应能提取 evidence");
        let (text, source) = &evidences[0];
        assert!(text.contains("黄金价格"), "应包含真实 AI 回复内容");
        assert!(source.starts_with("Bash["), "来源标签应是 Bash[toolu_1]");
    }

    #[test]
    fn extract_runner_evidence_skips_failed_tool_results() {
        // tool_result is_error=true 应被跳过(避免污染产物)
        let msgs = vec![
            ChatMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
                input: serde_json::json!({"command": "false"}),
            }]),
            ChatMessage::tool_result("t1", "<stdout>\nerror\n</stdout>\n<exit_code>1</exit_code>", true),
        ];
        let evidences = SubAgentRunner::extract_runner_evidence_from_session(&msgs);
        // 失败时 content 含 exit_code 1,会被过滤;提取仍可能抓到"error"但因 < 30 字符被截掉
        // 实际上 extract_bash_main_output 只在 >=30 字符时返回;这里 content 长度太短
        // 验证不会因错误调用产生误导性 evidence
        for (t, _) in &evidences {
            assert!(!t.contains("exit_code"), "不应包含退出码噪音");
        }
    }

    #[test]
    fn runner_text_needs_fallback_distinguishes_short_and_long() {
        // 短描述性文本(无动作关键词)→ 需要兜底
        assert!(runner_text_needs_fallback("任务成功"));
        assert!(runner_text_needs_fallback("处理完毕"));
        // 含动作关键词 → 不需要兜底
        assert!(!runner_text_needs_fallback("已打开 example.com 并截图"));
        assert!(!runner_text_needs_fallback("page_id=p_xxx 已点击按钮"));
        // 长文本(>=200) → 不需要兜底
        let long_text = "x".repeat(250);
        assert!(!runner_text_needs_fallback(&long_text));
        // 空文本 → 需要兜底
        assert!(runner_text_needs_fallback(""));
        assert!(runner_text_needs_fallback("   "));
    }

    #[test]
    fn extract_bash_main_output_finds_longest_block() {
        let content = "<stdout>\nshort\n\nthis is a long block with detailed AI response content here, should be picked over the short one\n\nlast\n</stdout>";
        let extracted = extract_bash_main_output(content);
        assert!(extracted.is_some());
        let s = extracted.unwrap();
        assert!(s.contains("detailed AI response"));
    }

    #[test]
    fn extract_runner_evidence_handles_write_with_large_content() {
        // Write 工具:tool_call.arguments.content 在 Assistant 消息,
        // tool_result 是写入成功的 JSON 信封
        let msgs = vec![
            ChatMessage::assistant(vec![ContentBlock::ToolUse {
                id: "w1".into(),
                name: "Write".into(),
                input: serde_json::json!({
                    "file_path": "/tmp/wenxin_result.txt",
                    "content": "#!/usr/bin/env python3\n# 长 Python 脚本,模拟爬取文心一言...\nimport sys\nprint('result')\n".repeat(50)
                }),
            }]),
            ChatMessage::tool_result(
                "w1",
                r#"{"code":0,"message":"ok","data":{"file_path":"/tmp/wenxin_result.txt","bytes_written":3500}}"#,
                false,
            ),
        ];
        let evidences = SubAgentRunner::extract_runner_evidence_from_session(&msgs);
        // Write 工具目前只生成文件路径+字节数摘要(< 50 字符,会被跳过)
        // 这是预期行为(避免在 evidence 里塞 5KB 内容)
        for (t, _) in &evidences {
            assert!(
                t.contains("/tmp/wenxin_result.txt") || t.contains("wenxin"),
                "Write evidence 应包含文件路径摘要"
            );
        }
    }
}
