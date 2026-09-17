//! Quality-Check Agent:质检层,对 SubAgent-Work / Main-Work / Plan 单元做质量校验。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::context::AgentRole;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::memory;
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::Result;
use crate::llm::{ChatMessage, Usage};
use crate::session;

/// 质检结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    Fail,
}

/// 质检报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityReport {
    pub verdict: Verdict,
    pub source: AgentRole,
    #[serde(default)]
    pub issues: Vec<String>,
    #[serde(default)]
    pub suggestion: String,
    pub retryable: bool,
    #[serde(default)]
    pub evidence: String,
}

impl QualityReport {
    pub fn pass(source: AgentRole) -> Self {
        Self {
            verdict: Verdict::Pass,
            source,
            issues: Vec::new(),
            suggestion: String::new(),
            retryable: false,
            evidence: String::new(),
        }
    }

    pub fn fail(source: AgentRole, issues: Vec<String>, suggestion: &str, retryable: bool) -> Self {
        Self {
            verdict: Verdict::Fail,
            source,
            issues,
            suggestion: suggestion.into(),
            retryable,
            evidence: String::new(),
        }
    }
}

/// Quality 执行器。
pub struct QualityRunner {
    agent: Agent,
    db: Arc<Db>,
}

impl QualityRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        let agent = Agent::new(llm, AgentProfile::quality_check_profile());
        Self { agent, db }
    }

    /// 校验 SubAgent-Work 单元输出。
    ///
    /// 2026-09-09 第 05 轮:接收 `ExecutionTrace` 作为辅助判据,让 QC 模型
    /// 既能看实际输出文本,也能看到真实执行证据(工具调用次数 / 失败 / 早终止),
    /// 避免「黑盒判定」误判 silently pass。
    ///
    /// 2026-09-09 第 14 轮:返回 `(QualityReport, Usage)`,把 Quality 调用的 LLM
    /// 用量带回给 Orchestrator 累加到 `total_usage`(修复 stdout 用量与 Debug 报告
    /// 不一致的 Bug)。
    pub async fn check_subagent(
        &self,
        goal: &str,
        unit_scope: &str,
        expected_output: &str,
        actual_output: &str,
        trace: &ExecutionTrace,
        session_id: &str,
    ) -> Result<(QualityReport, Usage)> {
        self.check_subagent_with_source(
            AgentRole::SubAgent,
            goal,
            unit_scope,
            expected_output,
            actual_output,
            trace,
            session_id,
        )
        .await
    }

    /// [`check_subagent`] 的来源角色参数化版本(2026-09-14 第 9 角色 WindowUse):
    /// delegate_to=windowuse 的 WorkFlow 单元质检时传 `AgentRole::WindowUse`,
    /// 让质检报告 source 与提示词标题反映真实执行角色。
    pub async fn check_subagent_with_source(
        &self,
        source: AgentRole,
        goal: &str,
        unit_scope: &str,
        expected_output: &str,
        actual_output: &str,
        trace: &ExecutionTrace,
        session_id: &str,
    ) -> Result<(QualityReport, Usage)> {
        let trace_summary = trace.render_prompt();
        let unit_label = match source {
            AgentRole::WindowUse => "WindowUse 单元(桌面窗口操控)",
            AgentRole::WebUse => "WebUse 单元(浏览器网页操控)",
            _ => "SubAgent 单元",
        };
        // F10(2026-09-10 第 25 轮):判定基准是「本单元职责」,整体目标仅作背景。
        // 此前 prompt 只给整体 goal,QC(真实 LLM)按整体目标判单元产物,SubAgent
        // 只完成了 wf-1 前置检查也被判「核心任务未完成」→ 无效重试风暴
        // (Debug 报告 debug_report_20260910_150805 问题报告 P1)。
        let prompt = format!(
            "【Quality-Check: {unit_label}】\n\
             整体目标(仅作背景,不作为本单元判定依据): {goal}\n\
             本单元职责(判定依据): {unit_scope}\n\
             本单元期望输出: {expected_output}\n\
             实际输出: {actual_output}\n\
             \n\
             【执行轨迹】\n{trace_summary}\n\
             \n\
             请基于「本单元职责 + 期望输出 + 实际输出 + 执行轨迹」判定本单元是否完成,**不要用整体目标苛求本单元**\
             (整体目标的其余部分由后续 WorkFlow 单元负责)。按 JSON 输出 verdict/source/issues/suggestion/retryable/evidence。\n\
             判定提示:若轨迹包含 early_terminate / high_error_rate / text_failure_phrase 信号,通常应判 Fail 并把对应信号写入 issues。\n\
             若本单元是“验证预期失败”的负例,底层 Bash 非零本身可能是通过条件;此时必须在 evidence 中说明预期性,并引用最终验收输出 EXPECTED_NEGATIVE_OK。",
        );
        self.run_check(prompt, source, actual_output, session_id, Some(trace))
            .await
    }

    /// 校验 Main-Work WorkFlow 计划(返回带 LLM Usage)。
    pub async fn check_main(
        &self,
        goal: &str,
        workflow_json: &str,
        session_id: &str,
    ) -> Result<(QualityReport, Usage)> {
        let prompt = format!(
            "【Quality-Check: Main-Work 单元】\n目标: {goal}\nWorkFlow JSON: {workflow_json}\n\n请按 JSON 格式输出 verdict/source/issues/suggestion/retryable/evidence。",
        );
        self.run_check(prompt, AgentRole::MainWork, workflow_json, session_id, None)
            .await
    }

    /// 校验 Plan Markdown(返回带 LLM Usage)。
    pub async fn check_plan(
        &self,
        plan_markdown: &str,
        session_id: &str,
    ) -> Result<(QualityReport, Usage)> {
        let prompt = format!(
            "【Quality-Check: Plan 单元】\nPlan Markdown:\n{plan_markdown}\n\n请按 JSON 格式输出 verdict/source/issues/suggestion/retryable/evidence。",
        );
        self.run_check(prompt, AgentRole::Plan, plan_markdown, session_id, None)
            .await
    }

    async fn run_check(
        &self,
        prompt: String,
        source: AgentRole,
        actual: &str,
        session_id: &str,
        trace: Option<&ExecutionTrace>,
    ) -> Result<(QualityReport, Usage)> {
        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;
        // P0:fail-closed —— 解析失败时返回 Verdict::Fail + retryable=true,
        // 触发 Yolo 回流,而非 fail-open(默认 Pass)绕过质检门。
        // 设计依据:`docs/Agent源码调研/专题-第八轮-Tool权限策略引擎与沙箱设计深度对比.md`
        // —— 5 态权限状态机 + fail-closed 默认;以及第十四轮 §1.2 错误恢复专题
        // —— silent pass 等于绕过熔断。
        let report = match parse_quality_report(&text, source) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    source = %source.as_str(),
                    "Quality 报告解析失败,fail-closed:返回 Verdict::Fail 触发回流"
                );
                // 落 Agent-Memory 记录原始输出长度,便于未来加入 JSON 修复链
                // (参考 atomcode repair.rs 8 段修复)
                let _ = memory::record_entry(
                    &self.db,
                    AgentRole::QualityCheck,
                    session_id,
                    "parse_failed",
                    &text,
                    Some(&format!("解析失败: {e}")),
                    serde_json::json!({ "raw_output_len": text.len() }),
                );
                QualityReport::fail(
                    source,
                    vec![format!("Quality 报告解析失败: {e}")],
                    "请确保 LLM 输出合法 JSON verdict/report 字段;建议重试或补充上下文。",
                    true,
                )
            }
        };
        let report = match trace {
            Some(trace) => gate_report_on_trace(report, source, trace, actual),
            None => report,
        };
        // 运行日志(2026-09-17 第 69 轮):Quality-Check 报告(决策)—— 全部 QC 入口
        // (单元/计划/Plan)汇聚本函数,一处记录 verdict/issues/建议/证据。
        tracing::info!(
            agent = "LsmAgentEmergentWork-Quality-Check",
            session = session_id,
            source = %source.as_str(),
            verdict = if report.verdict == Verdict::Pass { "pass" } else { "fail" },
            retryable = report.retryable,
            issues = %crate::logging::clip(&report.issues.join(" | ")),
            suggestion = %crate::logging::clip(&report.suggestion),
            evidence = %crate::logging::clip(&report.evidence),
            "Quality-Check 报告(决策)"
        );

        let _ = memory::record_entry(
            &self.db,
            AgentRole::QualityCheck,
            session_id,
            &format!("check source={}", source.as_str()),
            &format!("verdict={:?}", report.verdict),
            if report.verdict == Verdict::Fail {
                Some(&report.suggestion)
            } else {
                None
            },
            serde_json::json!({ "issues": &report.issues, "retryable": report.retryable }),
        );

        let _ = actual;
        Ok((report, usage))
    }
}

/// 用不可伪造的执行轨迹约束 Quality-Check 的“通过”结论。
///
/// LLM QC 是语义判断,但不能覆盖进程级事实:
/// - 进程级硬失败信号(early_terminate / high_error_rate)出现时,pass 一律降级为 fail;
/// - 文本级软信号(text_failure_phrase)降为“证据可豁免”(2026-09-11 第三十六轮 LA-3):
///   终答为提升可观测性引用工具输出摘录时,摘录中的 "AssertionError:"/"error:" 等日志
///   措辞会误触发该信号 —— 引用日志 ≠ 模型自己声称失败。QC 提供非空 evidence 说明
///   预期性,或预期负例契约确认(EXPECTED_NEGATIVE_OK)任一即可保持 pass;
/// - Bash 非零退出是弱信号:若 QC 想把它解释为预期负例,必须提供非空 evidence;
///   空 evidence 的 pass 视为无效质检,触发回流/失败收口。
///
/// 这样保留“命令预期失败”的测试空间,同时杜绝 D01Q4 这类 exit=1 被静默判成功。
fn gate_report_on_trace(
    report: QualityReport,
    source: AgentRole,
    trace: &ExecutionTrace,
    actual_output: &str,
) -> QualityReport {
    if report.verdict == Verdict::Fail {
        return report;
    }

    // 负例测试契约:底层命令非零是“通过条件”,最终断言必须返回零并输出
    // EXPECTED_NEGATIVE_OK。这样避免 QC 模型忘记填 evidence 时,把已经成功验证
    // 的预期负例反复重试;同时仍要求最终验收命令成功,防止用文字口头豁免。
    let expected_negative_confirmed = actual_output.contains("EXPECTED_NEGATIVE_OK")
        && trace.bash_exit_nonzero_count > 0
        && trace.last_bash_exit_code == 0;
    // 文本软信号单独判定:仅当不存在进程级硬信号时才允许证据豁免
    let has_text_failure_phrase = trace
        .failure_signals
        .iter()
        .any(|s| s == "text_failure_phrase");
    let has_hard_failure = trace
        .failure_signals
        .iter()
        .any(|s| s.starts_with("early_terminate:") || s.starts_with("high_error_rate:"));
    let text_phrase_only = has_text_failure_phrase && !has_hard_failure;
    let hard_strong_failure = trace.is_failed() && !text_phrase_only;
    let unevidenced_text_phrase =
        text_phrase_only && report.evidence.trim().is_empty() && !expected_negative_confirmed;
    let unevidenced_bash_failure = trace.bash_exit_nonzero_count > 0
        && report.evidence.trim().is_empty()
        && !expected_negative_confirmed;
    if !hard_strong_failure && !unevidenced_text_phrase && !unevidenced_bash_failure {
        return report;
    }

    let issue = if hard_strong_failure {
        // 2026-09-16 第 64 轮:WebUse 专项 issue 文案,根据 early_terminate_reason
        // 拆分子类,给出可执行诊断(此前 boilerplate 对用户无帮助)。
        if source == AgentRole::WebUse {
            let web_use_reason = trace
                .failure_signals
                .iter()
                .find(|s| s.starts_with("early_terminate:"))
                .cloned();
            match web_use_reason.as_deref() {
                // 2026-09-17 第 75 轮:delegate_mismatch 弱信号给出明确文案。
                // 典型场景:网页任务被路由到 WindowUseRunner,Runner 没 Browser* 工具,
                // 16 轮 0 工具调用。trace.failure_signals 会含 "delegate_mismatch:runner=WindowUse intended=WebUse"。
                _ if trace.failure_signals.iter().any(|s| s.starts_with("delegate_mismatch:runner=WindowUse intended=WebUse"))
                    => format!(
                        "WebUse 委派错配:WorkFlow 期望执行 Chromium-WebUse 工具集(BrowserNew/BrowserControl/BrowserInspect),\
                         但 Runner 实际是 WindowUse(无 Browser* 工具)。\
                         iter={} tool_calls={}。\
                         排查:1) 检查 Main-Work delegate_to 字段是否被 infer_delegate_to 纠正;\
                         2) 若纠正失败,检查 steps 是否含 WINDOW_USE_STRICT 桌面 GUI 强信号词被误命中;\
                         3) 跑下轮重试前确认 trace.runner_role=WebUse trace.intended_role=WebUse。",
                        trace.iterations, trace.tool_calls
                    ),
                Some(r) if r.contains("no_tool_use_no_action_text") => format!(
                    "WebUse 单元 {} 次迭代内未调用任何 BrowserNew/BrowserControl/BrowserInspect 工具,\
                     LLM 持续返回纯文本。trace 工具调用次数=0。\
                     排查:(a) 首步强制 BrowserNew 是否被 LAEW_FORCED_TOOLS=off 关闭;\
                     (b) use_js=true 是否触发 React onChange;\
                     (c) page_id 是否因 overflow 截断丢失(已修复为 prompt 尾部注入)。",
                    trace.iterations
                ),
                Some(r) if r.contains("no_text_converge") => format!(
                    "WebUse 单元连续 8 轮仅 tool_use 无 final_text,触发 no_text_converge 短路。\
                     iter={} tool_calls={}。建议:LLM 输出改为'短文本 + 工具调用'混合模式。",
                    trace.iterations, trace.tool_calls
                ),
                _ => format!(
                    "WebUse 单元 {} 次迭代内出现 {} 次失败({}%),early_terminate={}。\
                     排查:selector / page_id 流转 / 浏览器进程是否存活。",
                    trace.iterations,
                    trace.tool_calls_err,
                    if trace.tool_calls > 0 {
                        trace.tool_calls_err * 100 / trace.tool_calls
                    } else {
                        0
                    },
                    trace.early_terminate_reason,
                ),
            }
        } else if source == AgentRole::WindowUse {
            // 2026-09-16 第 66 轮 P1-6:WindowUse 专项诊断 —— 按 early_terminate_reason /
            // failure_signals 给出可执行排查路径,LLM 重试轮据此直接调整策略。
            let wu_reason = trace
                .failure_signals
                .iter()
                .find(|s| s.starts_with("early_terminate:"))
                .cloned();
            let joined = trace.failure_signals.join(",");
            let err_dist_hint = format!(
                "iter={} tool_calls={} err={} early_terminate={}",
                trace.iterations,
                trace.tool_calls,
                trace.tool_calls_err,
                trace.early_terminate_reason
            );
            match wu_reason.as_deref() {
                // 2026-09-17 第 75 轮:WindowUse 接到了 web 任务(委派错配反方向)。
                _ if trace.failure_signals.iter().any(|s| s.starts_with("delegate_mismatch:runner=WebUse intended=WindowUse"))
                    => format!(
                        "WindowUse 委派错配:WorkFlow 期望执行桌面窗口操控工具集(WindowList/WindowInspect/WindowAction),\
                         但 Runner 实际是 WebUse(无 Window* 工具)。\
                         iter={} tool_calls={}。\
                         排查:1) 确认任务目标是桌面应用(微信/钉钉等)而非网页;\
                         2) 若确实是桌面应用,delegate_to 字段被改判成 webuse 通常意味着 Yolo 误分类,应回退到 medium/hard 档或重新委派。",
                        trace.iterations, trace.tool_calls
                    ),
                Some(r) if r.contains("no_tool_use_no_action_text") => format!(
                    "WindowUse 单元 {} 次迭代内未调用任何 Window 工具,LLM 持续返回纯文本。{}\
                     排查:首步强制 WindowOpen 是否生效;任务是否被误委派(应 delegate_to=windowuse)。",
                    trace.iterations, err_dist_hint
                ),
                _ if joined.contains("-25211") || joined.contains("APIDisabled") => format!(
                    "WindowUse 单元失败:macOS 辅助功能未授权(-25211)。{}\
                     排查:系统设置→隐私与安全性→辅助功能勾选宿主终端并重开;\
                     或降级 Bash 白名单 osascript/cliclick/pbcopy 路径。",
                    err_dist_hint
                ),
                _ if joined.contains("-25212") || joined.contains("越界") || joined.contains("路径失效") => format!(
                    "WindowUse 单元失败:控件路径失效/越界(-25212),UI 已变化。{}\
                     排查:重新 WindowInspect 获取最新 path 后再 WindowAction,不要重复旧 path;\
                     滚动列表后所有 path 都会失效,必须重新检视。",
                    err_dist_hint
                ),
                _ => format!(
                    "WindowUse 单元 {} 次迭代内出现 {} 次失败,early_terminate={}。{}\
                     排查:WindowFind 匹配模式(contains→fuzzy)/ filter 同义词表 / \
                     scroll+重新检视 循环 / send_keys(enter) 发送链路。",
                    trace.iterations, trace.tool_calls_err, trace.early_terminate_reason, err_dist_hint
                ),
            }
        } else {
            "执行轨迹包含强失败信号,Quality-Check 的 pass 结论被 trace 证据门拒绝".to_string()
        }
    } else if unevidenced_text_phrase {
        "终答包含失败措辞(可能引用了工具日志摘录)且 Quality-Check 未提供非空 evidence 说明预期性"
            .to_string()
    } else {
        "Bash 命令非零退出且 Quality-Check 未提供非空 evidence 说明预期性".to_string()
    };
    let suggestion = if source == AgentRole::WebUse && hard_strong_failure {
        "下一轮重试时:1) 确认首步 BrowserNew 被调用(visible in TUI);\
         2) 输入文本框用 BrowserControl(action=input_text, use_js:true) 触发框架 onChange;\
         3) 等待对话用 BrowserInspect(info=image_urls) + 5s 轮询直到 reply 元素出现;\
         4) 若浏览器不存在返回 code=3001,如实告知用户安装 Chrome/Edge/Chromium。"
            .to_string()
    } else if source == AgentRole::WindowUse && hard_strong_failure {
        // 2026-09-16 第 66 轮 P1-6:WindowUse 专项重试指引
        "下一轮重试时:1) 首步 WindowOpen(query) 启动/激活应用并拿 window_id;\
         2) 列表中找目标条目优先用搜索框 set_text 定位,无搜索框再 scroll+重新 WindowInspect;\
         3) 目标名含 Unicode 上标(ᴬᴵᴬ)时 filter 直接写 ASCII 归一形(AIA);\
         4) 发送消息:输入框 set_text 后 send_keys(\"enter\"),或 click「发送」按钮;\
         5) -25211 未授权时把授权步骤写进最终回答,或改走 Bash 白名单 osascript。"
            .to_string()
    } else {
        "请修正命令或产物后重跑;若非零退出是预期负例,Quality-Check 必须在 evidence 中说明。"
            .to_string()
    };
    let mut gated = QualityReport::fail(source, vec![issue], &suggestion, true);
    gated.evidence = trace.render_prompt();
    gated
}

/// 解析 Quality JSON 输出。
/// 直接解析失败时自动走 JSON 修复链(`json_repair` Tier-1 语法修复);
/// 修复不了仍返回 Err(上轮 P0 fail-closed 语义不变,截断 JSON 刻意不补全)。
///
/// 2026-09-17 第 82+ 轮 P0-3:新增「文本关键词降级兜底」 —— 当 JSON 解析完全失败
/// 且文本含可识别关键词时,推断 verdict / retryable / source,避免 591s 任务因
/// Provider 网关截断 verdict 字段而整轮 fail。fail-closed 语义不变:仍优先返回
/// Err,关键词降级仅在 Err 之后作为**最后兜底**。
pub fn parse_quality_report(text: &str, source: AgentRole) -> Result<QualityReport> {
    if let Some(json_str) = extract_json_block(text) {
        return crate::agent::json_repair::try_parse::<QualityReport>(json_str).map_err(|diag| {
            crate::error::AgentError::Other(format!("Quality JSON 解析失败: {diag}"))
        });
    }
    if let Some(json_str) = extract_standalone_json(text) {
        let mut r: QualityReport =
            crate::agent::json_repair::try_parse(json_str).map_err(|diag| {
                crate::error::AgentError::Other(format!("Quality JSON 解析失败: {diag}"))
            })?;
        if r.source != source {
            r.source = source;
        }
        return Ok(r);
    }
    // 2026-09-17 第 82+ 轮 P0-3:JSON 完全无法提取 → 文本关键词降级。
    // 仅当 JSON 解析链 fail-closed 之后触发,且文本同时含「质检报告」上下文
    // 关键词 + verdict 强信号时才走降级。其它场景仍 Err(避免误判)。
    if let Some(report) = parse_quality_report_text_fallback(text, source) {
        tracing::warn!(
            source = %source.as_str(),
            text_len = text.len(),
            "Quality JSON 解析失败,走文本关键词降级(避免整轮 fail)"
        );
        return Ok(report);
    }
    Err(crate::error::AgentError::Other(
        "未找到合法的 Quality JSON".into(),
    ))
}

/// 2026-09-17 第 82+ 轮 P0-3:文本关键词降级。
///
/// 适用场景:Provider 网关 / 日志截断把 emit JSON 的 `verdict` 字段切掉,
/// 仅剩自然语言结论。LLM 通常会在 JSON 后追加一行「结论:通过 / 结论:失败」。
/// 启发式匹配:
/// - verdict 关键词:`通过/pass/✅` → Pass;`失败/fail/❌/错误/缺陷` → Fail
///   (Pass 强信号优先,否则判 Fail);
/// - retryable:含「重试/retry/建议再试/next_iter」 → true,否则 false;
/// - source:从枚举关键词匹配(yolo / plan / main / subagent / quality_check /
///   session_context / windowuse),否则用调用方传入的 source。
///
/// 返回 `Some(QualityReport)` 当且仅当 verdict 强信号命中(否则返回 None)。
fn parse_quality_report_text_fallback(text: &str, source: AgentRole) -> Option<QualityReport> {
    let lower = text.to_lowercase();
    // verdict:Pass 强信号优先(包含「通过」或"pass"或 ✅)
    let pass_signals = ["结论:通过", "verdict: pass", "通过", "✅", "qc pass"];
    let fail_signals = ["结论:失败", "verdict: fail", "失败", "❌", "qc fail", "错误", "存在缺陷"];
    let verdict = if pass_signals.iter().any(|s| lower.contains(&s.to_lowercase())) {
        Verdict::Pass
    } else if fail_signals.iter().any(|s| lower.contains(&s.to_lowercase())) {
        Verdict::Fail
    } else {
        return None; // 没有强信号,不降级,仍 Err
    };
    // retryable
    let retry_signals = ["可重试", "建议重试", "retryable: true", "retry", "再试"];
    let retryable = retry_signals.iter().any(|s| lower.contains(&s.to_lowercase()));
    // source:从文本识别 AgentRole 枚举;不命中则用调用方传入的 source
    let source_inferred = if lower.contains("windowuse") {
        AgentRole::WindowUse
    } else if lower.contains("quality_check") {
        AgentRole::QualityCheck
    } else if lower.contains("subagent") {
        AgentRole::SubAgent
    } else if lower.contains("main") {
        AgentRole::MainWork
    } else if lower.contains("plan") {
        AgentRole::Plan
    } else if lower.contains("yolo") {
        AgentRole::Yolo
    } else {
        source
    };
    // issues:截前 200 字符作为 evidence(供 Debug 报告观测)
    let evidence = text.chars().take(200).collect::<String>();
    Some(QualityReport {
        verdict,
        source: source_inferred,
        issues: vec![format!("JSON 解析失败,文本降级;evidence={evidence}")],
        suggestion: String::new(),
        retryable,
        evidence,
    })
}

fn extract_json_block(text: &str) -> Option<&str> {
    let start_marker = "```json";
    let start = text.find(start_marker)?;
    let content_start = start + start_marker.len();
    let content_start = text[content_start..]
        .find(|c: char| !c.is_whitespace())
        .map(|i| content_start + i)
        .unwrap_or(content_start);
    let end_marker = "```";
    let end = text[content_start..].find(end_marker)?;
    let json_text = &text[content_start..content_start + end].trim();
    if json_text.is_empty() {
        None
    } else {
        Some(json_text)
    }
}

fn extract_standalone_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut end = None;
    for (i, c) in text[start..].char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' && in_string {
            escape = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    end.map(|e| &text[start..e])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_report_pass_default() {
        let r = QualityReport::pass(AgentRole::SubAgent);
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::SubAgent);
    }

    #[test]
    fn quality_report_fail_sets_retryable() {
        let r = QualityReport::fail(AgentRole::SubAgent, vec!["x".into()], "再试一次", true);
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
        assert_eq!(r.suggestion, "再试一次");
    }

    #[test]
    fn parse_quality_report_from_block() {
        let text = r#"
```json
{
  "verdict": "pass",
  "source": "subagent",
  "issues": [],
  "suggestion": "",
  "retryable": false,
  "evidence": ""
}
```"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
    }

    // ========== P0 fail-closed 测试 ==========
    // 验证 JSON 解析失败时返回 Verdict::Fail + retryable=true
    // (替代历史的 fail-open 默认 Pass)

    #[test]
    fn parse_quality_report_no_json_fails_closed() {
        // 完全无 JSON → 应返回 Err(让上游 fail-closed 转为 Fail)
        let text = "这是一段不含 JSON 的自然语言,无法被解析为质检报告。";
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_truncated_json_fails_closed() {
        // JSON 被截断 / 不合法 → 应返回 Err
        let text = r#"以下是质检报告:
```json
{
  "verdict": "pass",
  "source": "subagent",
  "issues": [],
  // 后面被截断"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_missing_required_field_fails_closed() {
        // 缺 verdict 字段 → serde 反序列化失败 → Err
        let text = r#"```json
{
  "source": "subagent",
  "issues": [],
  "retryable": false
}
```"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("verdict"));
    }

    #[test]
    fn quality_report_fail_constructor_sets_retryable() {
        // 验证 fail-closed 路径所需的构造函数
        let r = QualityReport::fail(
            AgentRole::SubAgent,
            vec!["Quality 报告解析失败: 字段缺失".to_string()],
            "请确保 LLM 输出合法 JSON verdict/report 字段",
            true,
        );
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
        assert!(r.issues.iter().any(|i| i.contains("解析失败")));
        assert!(r.suggestion.contains("JSON"));
    }

    // ========== 2026-09-17 第 82+ 轮 P0-3 测试 ==========
    // 文本关键词降级:JSON 完全无法提取时,从正文关键词推断 verdict / retryable / source,
    // 避免 Provider 网关截断 verdict 字段导致整轮 fail。

    #[test]
    fn parse_quality_report_text_fallback_pass() {
        // 自然语言结论 + 「通过」关键词 → 降级 Pass
        let text = "经过核查,本单元所有子任务均完成,验收材料齐全,结论:通过";
        let r = parse_quality_report(text, AgentRole::WindowUse).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::WindowUse); // 文本含 windowuse
        assert!(r.issues[0].contains("JSON 解析失败"));
    }

    #[test]
    fn parse_quality_report_text_fallback_fail_with_retry() {
        // 含「失败」+「重试」 → 降级 Fail + retryable=true
        let text = "子任务超时,前置未达成,结论:失败,建议重试";
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
    }

    #[test]
    fn parse_quality_report_text_fallback_no_strong_signal_fails_closed() {
        // 无强信号关键词 → 仍 fail-closed(避免误判)
        let text = "今天天气很好,适合出门散步";
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_text_fallback_source_inference() {
        // 含「windowuse」→ 推断 source=WindowUse(即便传入其它角色)
        let text = "QC pass:windowuse 单元所有动作完成";
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::WindowUse);
    }

    #[test]
    fn trace_gate_rejects_empty_evidence_for_bash_nonzero() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 1;
        trace.collect_failure_signals("完成");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "完成",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.retryable);
        assert!(gated.evidence.contains("bash_exit_nonzero=1"));
    }

    #[test]
    fn trace_gate_allows_expected_bash_nonzero_with_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 2;
        trace.collect_failure_signals("负向用例按预期返回非零");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "exit=2 是本负向用例的预期结果".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "完成");

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_strong_failure_even_with_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.early_terminated = true;
        trace.early_terminate_reason = "max_iter:2".into();
        trace.collect_failure_signals("完成");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "模型解释这是预期情况".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "完成");

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("强失败信号"));
    }

    #[test]
    fn trace_gate_allows_explicit_negative_confirmation_without_qc_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 0;
        trace.collect_failure_signals("EXPECTED_NEGATIVE_OK");

        let output = "工具输出摘录\nEXPECTED_NEGATIVE_OK\n<exit_code>0</exit_code>";
        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            output,
        );

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_negative_marker_without_final_success() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 1;
        trace.collect_failure_signals("EXPECTED_NEGATIVE_OK");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "EXPECTED_NEGATIVE_OK",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("Bash 命令非零退出"));
    }

    #[test]
    fn trace_gate_allows_text_phrase_with_expected_negative_contract() {
        // LA-3:终答摘录引用了 AssertionError 日志(误触发 text_failure_phrase),
        // 但预期负例契约确认(先非零复现 + 最终断言命令 exit=0 + EXPECTED_NEGATIVE_OK)
        // 应保持 pass,不应反复回流重试。
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 0;
        trace.collect_failure_signals(
            "MOCK_FINAL_ANSWER: 验证通过。\n\n[工具输出摘录]\nAssertionError: producer error: 有 queue.Full 未处理",
        );
        assert!(trace
            .failure_signals
            .iter()
            .any(|s| s == "text_failure_phrase"));

        let output = "AssertionError 摘录\nEXPECTED_NEGATIVE_OK\n<exit_code>0</exit_code>";
        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            output,
        );

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_text_phrase_without_contract_or_evidence() {
        // LA-3:纯文本失败措辞(无契约确认、无 QC evidence)仍必须拒绝,
        // 防止模型口头上声称成功、引用失败日志蒙混过关。
        let mut trace = ExecutionTrace::default();
        trace.collect_failure_signals("执行失败: 无法完成队列写入");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "任务完成",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("失败措辞"));
    }

    #[test]
    fn trace_gate_allows_text_phrase_with_qc_evidence() {
        // LA-3:QC 用非空 evidence 说明"该失败措辞来自预期负例日志摘录"→ 放行
        let mut trace = ExecutionTrace::default();
        trace.collect_failure_signals("AssertionError: 预期复现摘录");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "终答中的 AssertionError 是负例脚本预期输出,断言已确认捕获".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "任务完成");

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_hard_failure_still_rejects_with_text_phrase_present() {
        // LA-3:early_terminate 与 text_failure_phrase 并存时按进程级硬信号无条件拒绝
        let mut trace = ExecutionTrace::default();
        trace.early_terminated = true;
        trace.early_terminate_reason = "max_iter:2".into();
        trace.collect_failure_signals("[MaxIterationsExceeded] 迭代达到 2 次上限未得到最终答案");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "模型解释这是预期情况".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "任务完成");

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("强失败信号"));
    }
}
