//! Yolo 分类封装与失败回流(2026-09-17 自 orchestrator.rs 拆分)。
//!
//! 分类入口封装 / 平台兜底提示 / 失败重分类 / 调试日志 / 失败事件落库,
//! 以及占位直答判定与用户兜底建议等自由函数。

use super::*;

impl MultiAgentOrchestrator {
    // ========== Yolo 分类 + 失败回流 ==========

    pub(super) async fn run_yolo_classification(
        &self,
        session: &Session,
    ) -> Result<(TaskClassification, Usage)> {
        // session_id 传播:Yolo 请求的 X-Session-Id 与任务主会话一致(抓包可关联,
        // 第 08 轮,方案 tmpPlan/2026-09-09_08)
        let (mut c, _text, usage) = self.yolo.classify(session.id(), session.context()).await?;
        // 修正:若 agent_role 缺省,按 task_level 推断
        if c.agent_role.is_none() {
            c.agent_role = Some(match c.task_level {
                TaskLevel::Simple => AgentRole::SubAgent,
                TaskLevel::Medium => AgentRole::MainWork,
                TaskLevel::Hard => AgentRole::Plan,
            });
        }
        Ok((c, usage))
    }

    /// 平台级 fallback 提示(2026-09-16 第 54 轮补丁 E)。
    ///
    /// 当 WindowUse 单元失败时,在失败原因中追加「当前平台 WindowUse 工具可用性 +
    /// 推荐替代路径」,让 Yolo 重新评估时拿到完整上下文,避免反复在死路上重试。
    /// 返回值限制 200 字符以内,避免撑爆 token。
    pub(super) fn platform_fallback_hint(&self, failure: &QualityFailure) -> String {
        // 仅当失败来源与 WindowUse 相关时才追加(其它场景避免噪声)
        let is_windowuse_related =
            matches!(failure.source, AgentRole::WindowUse | AgentRole::MainWork);
        if !is_windowuse_related {
            return String::new();
        }
        // 探测当前平台 WindowUse 工具可用性
        #[cfg(target_os = "macos")]
        {
            // 2026-09-16 第 55 轮修正:macOS 上 AX C API 全版本可用,「不可用」只有一种
            // 情况 = 辅助功能未授权(kAXErrorAPIDisabled)。通过 permission_hint 是否返回
            // Some 判定:已授权 → None(无需 fallback);未授权 → 给 Yolo 精简重试指引。
            let driver = crate::agent::window::current_driver();
            if driver.permission_hint().is_some() {
                return "[platform-fallback] macOS 辅助功能未授权,WindowInspect/WindowAction 暂不可用。建议:提示用户到 系统设置→隐私与安全性→辅助功能 勾选宿主终端并重开;或把含 osascript/screencapture/cliclick/System Events/keystroke 的步骤改 delegate_to=subagent(WindowUse Bash 已扩白名单);WindowList 走 CoreGraphics 始终可用。".to_string();
            }
            String::new()
        }
        #[cfg(windows)]
        {
            // Windows:若 WindowUse 失败,建议改 PowerShell + UI Automation
            let hint = std::env::var("LAEW_WINDOWS_UIA_FALLBACK")
                .ok()
                .filter(|v| !v.is_empty());
            if let Some(_) = hint {
                return "[platform-fallback] Windows UIA 不可用,建议改 PowerShell + System.Windows.Automation 路径。".to_string();
            }
            String::new()
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            // Linux:无 GUI 自动化时建议改 xdotool / wmctrl 完整命令模板
            let driver = crate::agent::window::current_driver();
            if driver.permission_hint().is_some() {
                return "[platform-fallback] 当前 Linux 平台 WindowUse 仅支持 wmctrl/xdotool 尽力而为,控件级操作常失败。建议:delegate_to=subagent 用 Bash 直接调 xdotool/wmctrl 命令模板。".to_string();
            }
            String::new()
        }
    }

    pub(super) async fn run_yolo_with_failure(
        &self,
        prev: &TaskClassification,
        failure: &QualityFailure,
        session: &mut Session,
    ) -> Result<TaskClassification> {
        // 2026-09-16 第 54 轮补丁 E:在失败原因末尾追加平台级 fallback 提示
        // (限制 200 字符以内,避免撑爆 token)
        let platform_hint = self.platform_fallback_hint(failure);
        let reason_with_hint = if platform_hint.is_empty() {
            failure.reason.clone()
        } else {
            // 截断原 reason,防止总长度超限
            const MAX_REASON_CHARS: usize = 400;
            let truncated_reason: String = failure.reason.chars().take(MAX_REASON_CHARS).collect();
            if truncated_reason.chars().count() == MAX_REASON_CHARS {
                format!(
                    "{truncated_reason}…
{platform_hint}"
                )
            } else {
                format!(
                    "{truncated_reason}
{platform_hint}"
                )
            }
        };

        // 构造失败摘要消息,让 Yolo 重新评估。
        // 2026-09-09 第 05 轮:把 ExecutionTrace 的 failure_signals 也带上,
        // 让 Yolo 看到具体失败模式而非仅凭自由文本判定。
        let failure_signals = failure
            .trace
            .as_ref()
            .map(|t| t.failure_signals.join(","))
            .unwrap_or_default();
        let failure_msg = format!(
            "[PREVIOUS_FAILURE]\n源: {}\n任务级别: {}\n原目标: {}\n失败原因: {}\n失败信号: {}\n建议: {}\n请重新评估:可重试 → 修订 decomposition_plan 重发;不可重试 → 填 user_suggestion_if_fail 并给出 direct_answer 告知用户。",
            failure.source.as_str(),
            prev.task_level.as_str(),
            prev.goal_summary,
            reason_with_hint,
            failure_signals,
            failure.suggestion,
        );
        session
            .context_mut()
            .push(crate::llm::ChatMessage::user(failure_msg));

        // 2026-09-09 第 14 轮:Yolo 失败回流时,Yolo 自身的 LLM 用量需要累加到
        // total_usage(原本被丢弃,导致失败回流的 token 也未计入)。
        let (c, yolo_usage) = self.run_yolo_classification(session).await?;
        // 透传:调用方负责把 yolo_usage 合并进 total_usage
        let _ = yolo_usage; // 失败回流场景的累加由 handle_inner 的 outer loop 处理
        Ok(c)
    }

    // ========== Debug 采集钩子(未开启时零开销) ==========

    pub(super) fn dbg_classify(&self, c: &TaskClassification) {
        if let Some(d) = &self.cfg.debug {
            d.record_classification(c);
        }
    }

    pub(super) fn dbg_qc(&self, report: &QualityReport) {
        if let Some(d) = &self.cfg.debug {
            d.record_quality(report);
        }
    }

    pub(super) fn dbg_task_end(&self, outcome: &str, usage: Usage) {
        if let Some(d) = &self.cfg.debug {
            d.record_task_end(outcome, usage);
        }
    }

    pub(super) fn record_failure_event(&self, session_id: &str, c: &TaskClassification, suggestion: &str) {
        let _ = self
            .db
            .insert_session_memory(&crate::config::SessionMemoryEntry {
                session_id: session_id.to_string(),
                role: AgentRole::Yolo,
                event_type: EventType::Failure,
                content: format!("目标: {}\n达到最大重试次数", c.goal_summary),
                usage_input: 0,
                usage_output: 0,
            });
        if !suggestion.is_empty() {
            let _ = self
                .db
                .insert_session_memory(&crate::config::SessionMemoryEntry {
                    session_id: session_id.to_string(),
                    role: AgentRole::SessionContext,
                    event_type: EventType::Suggestion,
                    content: suggestion.into(),
                    usage_input: 0,
                    usage_output: 0,
                });
        }
    }

}

/// 判断 `direct_answer` 是否为占位字符串(2026-09-10 第 27 轮 F12)。
///
/// 上下文:`yolo.rs::TaskClassification.direct_answer` 期望需要委派时填 JSON
/// `null`(→ `Option::None`),需要直答时填字符串答案。但实测发现部分上游 LLM
/// 误把字面量 `"null"`/`"None"`/`"NULL"` 当 JSON `null` 写入,反序列化后
/// 是 `Some("null")`,原 `filter(|a| !a.trim().is_empty())` 不会拒绝它,导致
/// 走 DirectAnswer 短路、TUI 直接打印"null"用户以为零输出。
///
/// 把 4 类常见占位归一为"未填",继续走 loop → run_simple 委派 SubAgent。
pub(super) fn is_placeholder_direct_answer(s: &str) -> bool {
    let t = s.trim();
    t.is_empty()
        || t.eq_ignore_ascii_case("null")
        || t.eq_ignore_ascii_case("none")
        || t.eq_ignore_ascii_case("nil")
}

/// 当 Yolo 没有给出 user_suggestion 时,根据累计 usage 给出 actionable 兜底建议。
///
/// 关联报告: 2026-09-09_05 E-003。当前仅按 output token(代表 LLM 实际产出)
/// 启发式区分:
/// - output_token = 0 → LLM 完全没产出,提示「任务描述不够具体」;
/// - output_token > 0 → LLM 产出了文本但未触发成功判定,提示「可能需要拆分 / 改路径」。
pub(super) fn fallback_suggestion(total_usage: &Usage) -> String {
    if total_usage.output_tokens == 0 {
        // 完全无产出:任务描述可能不明确,或被早终止
        "未产出任何答复:请补充任务信息(目标 / 验收标准 / 输入数据)或调整目标粒度,\
         让 LLM 能给出明确的输出"
            .to_string()
    } else if total_usage.output_tokens < 50 {
        // 产出极少:可能被无文本收敛短路,或 LLM 反复试探
        "答复信息密度极低:可能因反复工具调用未收敛。请把任务描述得更具体(目标 / 输入 / \
         期望产出),或确认工具调用所需的资源(文件路径 / 环境)是否可达"
            .to_string()
    } else {
        // 已有较多产出但仍失败:通常是质量判定 / 路径错误类
        // F11(2026-09-10 第 25 轮):此前误把 output_tokens 当「迭代次数」展示
        // ("已迭代 398 次"),数值来自 token 统计,严重误导。改为如实描述。
        format!(
            "任务累计产出 {} output tokens 仍未通过质量判定:请确认任务目标是否合理、\
             工具调用结果是否正确,或拆分成更小的子任务",
            total_usage.output_tokens
        )
    }
}
