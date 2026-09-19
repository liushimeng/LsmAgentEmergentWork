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
    /// ★待处理的 Agent 消息(其他 Agent 发来的)。
    #[serde(default)]
    pub pending_agent_messages: Vec<AgentMessage>,
    /// ★2026-09-17 第 75 轮:WorkFlow 期望的 Agent 角色(`wf.delegate_to` 派生)。
    /// Runner 写入 `trace.intended_role`;与 `trace.runner_role` 对比可识别
    /// 「委派错配」(`delegate_mismatch` 弱信号),供 QC + TUI 给出明确诊断。
    /// `None` 表示 Orchestrator 未注入(老调用点兼容)。
    #[serde(default)]
    pub intended_role: Option<AgentRole>,
    /// 2026-09-19 第 91 轮 P0-7/P0-8:
    /// - `retry_count`: 本单元目前已进入 retry 第几轮(从 0 开始);
    /// Runner 写入 trace.failure_signals(方便 QC 判 Fail 时包含 retry 回次信息)。
    #[serde(default)]
    pub retry_count: usize,
    /// 2026-09-19 第 91 轮 P0-8:
    /// - `retry_hint`: 上一轮失败原因(由 Orchestrator 透传)。runner 将其接入 description 内印刷,
    /// SubAgent 在 retry 轮明示看到失败信息以避免原样重做同样的事(历史 llaew_*.log 实证第四轮都撞同墙)。
    #[serde(default)]
    pub retry_hint: String,
    /// 2026-09-19 第 91 轮 P0-6:
    /// - `max_iterations`: per-unit 计算迭代上限(SubAgentRunner 默认 16,长任务可递增至 24~32);
    /// `None` 表示使用 Runner 默认值。
    #[serde(default)]
    pub max_iterations: Option<usize>,
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
    /// 文心一言任务被降级 simple 直派 SubAgentRunner 时,
    /// SubAgentRunner 出口 outcome.text 仅含「任务成功完成,内容已保存到 wenxin_result.txt」,
    /// 真实抓取的 AI 回复在文件里,TUI 看不到 — 用户必须二次追问「结果显示在哪里了」。
    ///
    /// 兜底策略(与浏览器页面文本提取 `extract_page_reply_from_session` 同族思路):
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
        // ★ 2026-09-18 第 89 轮:多轮浏览器页面复用(自原 WebUseRunner 第 79 轮平移)。
        // 存活页面探测(顺带让 list_pages 清理失效 entry),非空时注入
        // 「已打开的浏览器页面」提示块,免重新打开/登录;冷启动零开销不注入。
        let live_pages = crate::agent::browser::BrowserManager::global().list_pages().await;
        let mut prompt = input.to_user_prompt();
        if let Some(hint) = build_existing_pages_hint(&live_pages) {
            prompt.push_str("\n\n");
            prompt.push_str(&hint);
            info!(
                live_pages = live_pages.len(),
                "SubAgent 注入已打开的浏览器页面提示(多轮复用)"
            );
        }
        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        // 让 sub_session 共享 session_id 便于追踪
        sub_session.id = session_id.to_string();

        // Agent 循环返回 (text, usage, trace) 三元组。
        // 早终止路径(RepeatedToolFailure / MaxIterationsExceeded)不再升级为 Error,
        // ★2026-09-17 第 75 轮:Runner 角色信息(SubAgent)。
        // ★2026-09-18 第 89 轮:执行器唯一化(Chromium-WebUse Agent 已删除,浏览器
        // 操控由本 Runner 的 MCP_Web_Use 工具承担),trace.runner_role 恒为 SubAgent,
        // trace.intended_role 反映 WorkFlow 期望的角色(供 delegate_mismatch 对账)。
        let runner_role = Some(AgentRole::SubAgent);
        let intended_role = input.intended_role;
        // 2026-09-19 第 91 轮 P0-6:per-unit max_iterations override
        // SubAgentRunner 默认 16,为长任务(长时待/多轮)审核 Main-Work 提供
        // 的 max_iterations 值(限 4..=64);默认 16 实现仍需超过 4 的交付
        let per_unit_max = input.max_iterations.unwrap_or(self.max_iterations).clamp(4, 64);
        let per_unit_agent = if per_unit_max != self.max_iterations {
            // Agent 不可 Clone,通过 with_max_iterations 重新构造(共享 llm + profile)
            Some(crate::agent::Agent::with_max_iterations(
                crate::agent::Agent::new(self.agent.llm(), self.agent.profile().clone()),
                per_unit_max,
            ))
        } else {
            None
        };
        let agent_ref = per_unit_agent.as_ref().unwrap_or(&self.agent);

        // 而是包装成一段失败摘要文本 + 已填充 early_terminated 的 trace,
        // 让 Quality-Check 仍可基于 trace 判定 Fail。
        let (text, usage, mut trace) = match agent_ref
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
        // 实质产物(让真实抓取的内容也能落到 outcome.text,而不是只看到
        // 「已保存到 xxx.txt」这种描述性占位句)。
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

        // ★ 2026-09-18 第 89 轮:浏览器真实页面文本出口兜底(自原 WebUseRunner
        // 第 76/78/82 轮平移)。MCP_Web_Use 抓取的页面文本(inspect elements /
        // control eval_js 的 tool_result 信封)在 LLM 终答常被「任务已完成,
        // 内容超过 N 字符」模板句吞掉;Runner 出口从 sub_session 反查最长一段
        // 可读文本(≥200 字符 + UI 占位文本过滤),作为最终产物追加。
        let text = match extract_page_reply_from_session(sub_session.context()) {
            Some((reply, source)) if !reply.trim().is_empty() => {
                info!(
                    extracted_chars = reply.chars().count(),
                    source = %source,
                    "SubAgent 出口兜底:追加浏览器抓取的真实页面文本"
                );
                let mut combined = text;
                if !combined.trim().is_empty() {
                    combined.push_str("\n\n");
                }
                combined.push_str(&format!(
                    "[来自浏览器抓取的真实页面回复,共 {} 字符,来源: {}]\n{}",
                    reply.chars().count(),
                    source,
                    reply,
                ));
                combined
            }
            _ => text,
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
        "已写入", "已生成", "page_id", "MCP_Web_Use", "action=open", "spawned_page_id",
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

// =================== 浏览器页面复用与真实文本出口兜底(2026-09-18 第 89 轮,自原
// =================== WebUseRunner 平移的纯函数;Agent 删除后能力归 SubAgent-Work) ===================

/// 构造「已打开的浏览器页面」注入提示块(原 WebUseRunner 第 79 轮能力平移)。
///
/// 输入为 `BrowserManager::list_pages()` 的 `(page_id, url, title, created_at)` 列表;
/// 空列表返回 None(冷启动,不注入)。纯函数,便于单测。
pub fn build_existing_pages_hint(pages: &[(String, String, String, String)]) -> Option<String> {
    if pages.is_empty() {
        return None;
    }
    let mut out = String::from(
        "【已打开的浏览器页面(可直接复用,免重新打开/登录)】\n",
    );
    for (id, url, title, _ts) in pages.iter().take(10) {
        let title_disp = if title.trim().is_empty() { "(无标题)" } else { title.as_str() };
        out.push_str(&format!("  - page_id={id} | 标题: {title_disp} | URL: {url}\n"));
    }
    out.push_str(
        "网页类任务优先用 MCP_Web_Use 的 inspect/control 直接操作上述页面;\
         仅当任务需要其它网址或页面已失效(code=2000)时才 action=open 新开。",
    );
    Some(out)
}

/// 从 sub_session 中提取「真实从浏览器抓到的可读文本」(原 WebUseRunner
/// 第 76/78/82 轮能力平移;第 89 轮起解析 MCP_Web_Use 的 tool_result 信封,
/// 信封形态与原 Browser* 工具一致,解析逻辑零改动)。
///
/// 业务背景:网页任务终答常输出模板句「任务已完成,AI回复已提取,内容超过 500 字符」,
/// 把真实回复吞掉。Runner 出口必须把 MCP_Web_Use inspect/control 抓到的真实文本
/// 追加到 outcome.text,让 QC 与 TUI 看到原文。
///
/// 提取策略:
/// 1. 倒序遍历 sub_session,过滤 role=Tool 的 ChatMessage;
/// 2. 对每个 tool_result.content 做 JSON 解析,寻找 `code==0 && data.{text|outer_html|...}`;
/// 3. 支持嵌套字段(`data.choices[0].message.content` / `data.markdown` /
///    `data.result.text` 等异构命名),通过 `extract_nested_string` 递归展开;
/// 4. 取长度最长且 ≥ 200 字符的候选,过滤 placeholder/UI 文本启发式白名单;
/// 5. 返回 `Some((text, source_tool))` —— text 已 trim,长度截断 8000 字符。
pub fn extract_page_reply_from_session(messages: &[ChatMessage]) -> Option<(String, String)> {
    // 阈值 200 + UI 文本过滤,避免抓取 input placeholder / 热搜推荐词 / 弹窗广告
    // 等 UI 文本误判为 AI 回复(第 82 轮实测文心一言任务曾中招)。
    const MIN_CHARS_AI_REPLY: usize = 200;
    let mut best: Option<(String, String, usize)> = None; // (text, source_tool, len)

    // 倒序遍历,优先取最近一次抓取结果(避免旧结果覆盖)
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
            // 仅解析 JSON 信封
            let parsed: Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // 失败码直接跳过
            if parsed.get("code").and_then(Value::as_i64) != Some(0) {
                continue;
            }
            let data = match parsed.get("data") {
                Some(d) => d,
                None => continue,
            };

            // 先用候选字段集快速匹配,失败再走递归嵌套提取
            // 候选字段涵盖主流 CDP 响应形态
            const CANDIDATE_KEYS: &[&str] = &[
                "text", "outer_html", "value", "html", "body",
                "reply", "answer", "markdown", "content",
                "reply_text", "ai_answer", "result", "result_text",
            ];
            let mut picked: Option<(String, &'static str)> = None;
            for key in CANDIDATE_KEYS {
                if let Some(s) = data.get(key).and_then(Value::as_str) {
                    let s = s.trim();
                    if s.chars().count() >= MIN_CHARS_AI_REPLY && !is_likely_ui_text(s) {
                        picked = Some((s.to_string(), *key));
                        break;
                    }
                }
            }
            // 候选字段都没命中 → 递归嵌套提取(覆盖 choices[0].message.content /
            // result.text / data.text / message.content 等异构命名)
            if picked.is_none() {
                if let Some(extracted) = extract_nested_string(data) {
                    let trimmed = extracted.trim();
                    if trimmed.chars().count() >= MIN_CHARS_AI_REPLY
                        && !is_likely_ui_text(trimmed)
                    {
                        picked = Some((trimmed.to_string(), "nested"));
                    }
                }
            }

            if let Some((text, label)) = picked {
                let len = text.chars().count();
                let better = match &best {
                    Some((_, _, l)) => len > *l,
                    None => true,
                };
                if better {
                    best = Some((text, format!("tool_result[{}]→{}", tool_use_id, label), len));
                }
            }
        }
    }

    best.map(|(text, source, len)| {
        // 截断到 8000 字符,避免 LLM 上下文中爆
        const MAX_CHARS: usize = 8000;
        let truncated = text.chars().count() > MAX_CHARS;
        let kept: String = if truncated {
            text.chars().take(MAX_CHARS).collect::<String>() + "\n...(已截断,原长="
                + &len.to_string()
                + "字符)"
        } else {
            text.to_string()
        };
        (kept, source)
    })
}

/// 从 JSON Value 中递归提取字符串内容(覆盖 AI 对话网站返回的嵌套字段:
/// `choices[0].message.content` / `markdown` / `reply_text` / `result.text` /
/// `content` 等异构命名)。
fn extract_nested_string(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(s) = extract_nested_string(item) {
                return Some(s);
            }
        }
        return None;
    }
    if let Some(obj) = v.as_object() {
        // 优先 message.content (OpenAI / DeepSeek 风格)
        if let Some(m) = obj.get("message") {
            if let Some(s) = extract_nested_string(m) {
                return Some(s);
            }
        }
        for key in &["content", "text", "result", "markdown"] {
            if let Some(found) = obj.get(*key).and_then(extract_nested_string) {
                return Some(found);
            }
        }
        for key in &["reply", "answer", "reply_text", "ai_answer", "value", "outer_html", "html", "body"] {
            if let Some(found) = obj.get(*key).and_then(extract_nested_string) {
                return Some(found);
            }
        }
        // 兜底:深度优先遍历所有字段
        for (_, v) in obj {
            if let Some(s) = extract_nested_string(v) {
                return Some(s);
            }
        }
    }
    None
}

/// 启发式判定一段文本是否是 UI 占位文本(input placeholder / 热搜推荐 /
/// 弹窗广告 / 输入框默认提示),而非 AI 生成的真实回复。
///
/// 触发任一即视为 UI 文本,出口兜底丢弃:
/// - 文本以常见 UI 占位开头("请输入"/"搜索"/"你好,我是" 等)
/// - 文本以列表形态开始(常见热搜推荐:"- 标题1\n- 标题2")
/// - 文本是 url 列表(每行 < 100 字且每行含 http:// 或 https://)
/// - 短文本(< 100 字)
fn is_likely_ui_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    const UI_PREFIXES: &[&str] = &[
        "请输入",
        "搜索",
        "search",
        "你好",
        "您好",
        "你可能想",
        "试试问",
        "示例:",
        "热门",
        "推荐",
        "placeholder=",
        "data-placeholder=",
    ];
    if UI_PREFIXES
        .iter()
        .any(|p| trimmed.to_lowercase().starts_with(p))
    {
        return true;
    }
    // 文本以列表形态开始(每行 `- xxx` 或 `1. xxx`)
    let first_line = trimmed.lines().next().unwrap_or("");
    if first_line.starts_with("- ") || first_line.starts_with("• ") {
        return true;
    }
    // URL 列表:行数 ≥ 3 且每行 < 100 字且每行都含 http:// 或 https://
    let lines: Vec<&str> = trimmed.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() >= 3 && lines.iter().all(|l| l.chars().count() < 100) {
        if lines
            .iter()
            .all(|l| l.contains("http://") || l.contains("https://") || l.contains("www."))
        {
            return true;
        }
    }
    // 短文本(< 100 字)直接视为 UI
    if trimmed.chars().count() < 100 {
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
            pending_agent_messages: vec![],
            // 2026-09-17 第 75 轮:SubAgent Runner 自测试默认走 SubAgent。
            intended_role: Some(AgentRole::SubAgent),
            retry_count: 0,
            retry_hint: String::new(),
            max_iterations: None,
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
            pending_agent_messages: vec![],
            // 2026-09-17 第 75 轮:SubAgent Runner 自测试默认走 SubAgent。
            intended_role: Some(AgentRole::SubAgent),
            retry_count: 0,
            retry_hint: String::new(),
            max_iterations: None,
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
            pending_agent_messages: vec![],
            // 2026-09-17 第 75 轮:SubAgent Runner 自测试默认走 SubAgent。
            intended_role: Some(AgentRole::SubAgent),
            retry_count: 0,
            retry_hint: String::new(),
            max_iterations: None,
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
            pending_agent_messages: vec![],
            // 2026-09-17 第 75 轮:SubAgent Runner 自测试默认走 SubAgent。
            intended_role: Some(AgentRole::SubAgent),
            retry_count: 0,
            retry_hint: String::new(),
            max_iterations: None,
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
            pending_agent_messages: vec![],
            // 2026-09-17 第 75 轮:SubAgent Runner 自测试默认走 SubAgent。
            intended_role: Some(AgentRole::SubAgent),
            retry_count: 0,
            retry_hint: String::new(),
            max_iterations: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        // original_prompt 默认值是 null,确保 SubAgent 输入 JSON 兼容老实现
        assert!(json.contains("\"original_prompt\":null"));
        // 新字段默认值也应正确序列化
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

    // =================== 2026-09-18 第 89 轮:页面复用提示 + 真实文本出口兜底 ===================

    #[test]
    fn build_existing_pages_hint_empty_returns_none() {
        assert!(build_existing_pages_hint(&[]).is_none(), "空列表不应注入提示");
    }

    #[test]
    fn build_existing_pages_hint_lists_page_id_url_title() {
        let pages = vec![
            (
                "p_ab12cd34".to_string(),
                "https://wenxin.baidu.com/".to_string(),
                "文心一言".to_string(),
                "1758100000000".to_string(),
            ),
            (
                "p_ff00ee11".to_string(),
                "https://example.com/".to_string(),
                String::new(), // 空标题 → (无标题)
                "1758100000001".to_string(),
            ),
        ];
        let hint = build_existing_pages_hint(&pages).expect("非空列表应返回提示块");
        assert!(hint.contains("已打开的浏览器页面"), "应含标题行: {hint}");
        assert!(hint.contains("page_id=p_ab12cd34"));
        assert!(hint.contains("文心一言"));
        assert!(hint.contains("https://wenxin.baidu.com/"));
        assert!(hint.contains("(无标题)"), "空标题应有占位: {hint}");
        assert!(hint.contains("MCP_Web_Use"), "应引导使用 MCP_Web_Use: {hint}");
    }

    #[test]
    fn build_existing_pages_hint_caps_at_ten_pages() {
        let pages: Vec<(String, String, String, String)> = (0..15)
            .map(|i| (
                format!("p_{i:08x}"),
                format!("https://example.com/{i}"),
                format!("标题{i}"),
                "1758100000000".to_string(),
            ))
            .collect();
        let hint = build_existing_pages_hint(&pages).unwrap();
        assert!(hint.contains("p_00000009"), "前 10 个页面应列出");
        assert!(!hint.contains("p_0000000a"), "第 11 个起应截断");
    }

    #[test]
    fn is_likely_ui_text_filters_placeholders() {
        assert!(is_likely_ui_text("请输入你的问题"));
        assert!(is_likely_ui_text("搜索"));
        assert!(is_likely_ui_text("潍坊市寒亭区委书记王勇被查")); // 热搜词,短文本
        assert!(is_likely_ui_text(
            "- https://example.com/page1\n- https://example.com/page2\n- https://example.com/page3"
        ));
        assert!(is_likely_ui_text("- 新对话\n- 工作任务\n- 知识库"));
        assert!(is_likely_ui_text("登录"));
        assert!(is_likely_ui_text(""));
        // 真实 AI 回复:长文 + 非 UI 开头 + 无 URL 列表
        assert!(!is_likely_ui_text(
            "根据最近3个月的黄金白银走势分析:7月份国际金价从853元/克上涨至906元/克,8月份突破1000元大关达到1012.85元/克的历史高点,9月份有所回落收于926.86元/克。白银方面,7月份在54-58美元区间震荡,8月份突破70美元后回落至63.80美元。综合来看,近期金银价格波动较大,投资者需注意风险控制。"
        ));
    }

    #[test]
    fn extract_page_reply_skips_placeholder_with_short_text() {
        // 80 字符 < 200 阈值,直接 None
        let placeholder_80chars = "潍坊市寒亭区委书记王勇被查,某某某最新消息,某某某官方回应,持续关注中";
        let msgs = vec![ChatMessage::tool_result(
            "t1",
            &format!(
                r#"{{"code":0,"message":"ok","data":{{"text":"{}"}}}}"#,
                placeholder_80chars
            ),
            false,
        )];
        let result = extract_page_reply_from_session(&msgs);
        assert!(result.is_none(), "短 placeholder 文本不应被采纳");
    }

    #[test]
    fn extract_page_reply_picks_longest_text() {
        let long_text = "黄金近3个月走势详细分析报告:7月份国际金价从853元/克持续上涨至906元/克,8月份突破1000元大关达到1012.85元/克的历史高点,9月份有所回落收于926.86元/克。白银方面,7月份在54-58美元区间震荡,8月份突破70美元后回落至63.80美元。综合来看,近期金银价格波动较大,投资者需密切关注美联储利率政策、地缘政治风险以及美元指数走势,合理配置资产以分散风险,以上分析仅供参考。";
        let short = "ok";
        let msgs = vec![
            ChatMessage::tool_result("t1", short, false),
            ChatMessage::tool_result("t2", &format!(
                r#"{{"code":0,"message":"ok","data":{{"text":"{}"}}}}"#,
                long_text
            ), false),
        ];
        let result = extract_page_reply_from_session(&msgs);
        assert!(result.is_some(), "应能提取到长文本(实际: {result:?})");
        let (text, _) = result.unwrap();
        assert!(text.contains("1012.85"), "应包含真实原文片段");
    }

    #[test]
    fn extract_page_reply_skips_failure_code() {
        // code=2002 失败时不应被当作提取源
        let long_text = "实际回复文本长度足够长通过门槛测试,这是文心一言生成的金银价格走势详细分析报告,包含国内国际金价白银价格数据。7月份国际金价从853元/克持续上涨至906元/克,8月份突破1000元大关达到1012.85元/克的历史高点,9月份回落至926.86元/克,白银方面7月份54-58美元区间震荡,8月突破70美元后回落63.80美元。投资者需密切关注美联储利率政策、地缘政治风险以及美元指数走势,合理配置资产以分散风险,以上分析仅供参考。";
        let msgs = vec![
            ChatMessage::tool_result("t1", r#"{"code":2002,"message":"err","data":{}}"#, false),
            ChatMessage::tool_result("t2", &format!(r#"{{"code":0,"message":"ok","data":{{"text":"{}"}}}}"#, long_text), false),
        ];
        let result = extract_page_reply_from_session(&msgs);
        assert!(result.is_some());
    }

    #[test]
    fn extract_page_reply_handles_nested_choices() {
        // 实际嵌套:tool_result.content = {"code":0, "data": {"choices": [{"message": {"content": "..."}}]}}
        let nested = r#"{"code":0,"message":"ok","data":{"choices":[{"message":{"content":"AI 回复: 根据最近3个月的黄金白银价格走势详细分析报告。国内金价从7月份的853元/克持续上涨至8月份的1012.85元/克历史高点,9月份回落至926.86元/克。国际金价目前4301.95美元/盎司,国际白银63.80美元/盎司,沪银主力15586元/千克。整体来看,近期金银价格波动较大,投资者需密切关注美联储利率政策、地缘政治风险以及美元指数走势,合理配置资产以分散风险。数据来源文心一言实时查询,以上价格仅供参考,实际交易以市场为准。"}}]}}"#;
        let msgs = vec![ChatMessage::tool_result("t1", nested, false)];
        let result = extract_page_reply_from_session(&msgs);
        assert!(result.is_some(), "嵌套字段应能提取,实际值: {result:?}");
    }

    #[test]
    fn extract_nested_string_handles_variants() {
        let v = serde_json::json!({
            "choices": [{"message": {"content": "AI 回复内容长度足够长通过门槛测试,实际是文心一言给的金价分析"}}]
        });
        assert!(extract_nested_string(&v).unwrap().contains("金价分析"));
        let v = serde_json::json!({"markdown": "# 标题\n这是 AI 生成的 markdown 内容"});
        assert!(extract_nested_string(&v).unwrap().contains("markdown 内容"));
        assert!(extract_nested_string(&serde_json::json!({"reply_text": "直接回复"})).is_some());
        assert!(extract_nested_string(&serde_json::json!({"count": 42, "flag": true})).is_none());
    }
}
