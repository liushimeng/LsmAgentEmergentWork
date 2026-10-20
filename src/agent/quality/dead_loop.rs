//! 死循环根因裁决通道(第 176 轮):Quality-Check 对「为什么循环、下一轮换哪条路」的结构化裁决。
//!
//! 本文件自 `quality.rs` 机械搬移而来(工程 1800 行拆分规范:代码逐行搬移零改写),
//! 仅新增本模块文档头与 `use super::*`。

use super::*;

impl QualityRunner {
    /// 死循环根因裁决通道(第 176 轮)。
    ///
    /// 与 [`check_subagent_with_source`](Self::check_subagent_with_source) 的分工:
    /// 后者判「产物对不对」,本方法判「**为什么循环、下一轮该换哪条路**」。
    ///
    /// 只在 `trace.dead_loop.is_some()` 时调用 —— 单元已被守卫止损,再来一次
    /// LLM 调用换取结构化的换路径建议,相对已烧掉的迭代预算可以忽略;
    /// 未命中死循环的单元零成本零开销。
    ///
    /// **fail-open 语义**(与 `run_check` 的 fail-closed 相反,且是刻意的):
    /// 本通道是**建议**不是**闸门** —— 单元此刻已经在失败路径上,裁决通道
    /// 自己再失败不应该把失败面扩大。解析失败时回落到机械默认值并如实留痕。
    pub async fn check_dead_loop(
        &self,
        unit_scope: &str,
        expected_output: &str,
        actual_output: &str,
        diag: &crate::agent::dead_loop::DeadLoopDiagnosis,
        session_id: &str,
    ) -> Result<(LoopResolution, Usage)> {
        let prompt = build_dead_loop_prompt(unit_scope, expected_output, actual_output, diag);
        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();
        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;
        let resolution = match parse_loop_resolution(&text) {
            Some(r) => r,
            None => {
                tracing::warn!(
                    session = session_id,
                    raw_len = text.len(),
                    "死循环根因裁决解析失败,fail-open:回落到机械默认建议"
                );
                LoopResolution::mechanical_fallback(diag)
            }
        };
        tracing::info!(
            agent = "LsmAgentEmergentWork-Quality-Check",
            session = session_id,
            role = "dead_loop_root_cause",
            root_cause = resolution.root_cause.as_str(),
            retryable = resolution.retryable,
            retry_path = %crate::logging::clip(&resolution.retry_path),
            rationale = %crate::logging::clip(&resolution.rationale),
            "死循环根因裁决(决策)"
        );
        let _ = memory::record_entry(
            &self.db,
            AgentRole::QualityCheck,
            session_id,
            "check role=dead_loop_root_cause",
            &format!("root_cause={}", resolution.root_cause.as_str()),
            if resolution.retryable {
                Some(&resolution.retry_path)
            } else {
                None
            },
            serde_json::json!({
                "diagnosis": diag.headline(),
                "retryable": resolution.retryable,
            }),
        );
        Ok((resolution, usage))
    }
}

/// 死循环根因枚举(与 QC 提示词硬门段要求的枚举一一对应)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopRootCause {
    /// 同一动作原地打转:换工具 / 换参数 / 换路径即可。
    SameAction,
    /// 目标不可达:重试无用,应换目标层级。
    SameTarget,
    /// 上下文自锁:模型在复述,应压缩上下文重新下达指令或直接收口。
    ContextStuck,
    /// 预算被无效动作烧光但方向正确:砍步骤,只做最关键的一件事。
    BudgetStarved,
}

impl LoopRootCause {
    pub fn as_str(self) -> &'static str {
        match self {
            LoopRootCause::SameAction => "same_action",
            LoopRootCause::SameTarget => "same_target",
            LoopRootCause::ContextStuck => "context_stuck",
            LoopRootCause::BudgetStarved => "budget_starved",
        }
    }

    pub(super) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "same_action" | "sameaction" => Some(LoopRootCause::SameAction),
            "same_target" | "sametarget" => Some(LoopRootCause::SameTarget),
            "context_stuck" | "contextstuck" => Some(LoopRootCause::ContextStuck),
            "budget_starved" | "budgetstarved" => Some(LoopRootCause::BudgetStarved),
            _ => None,
        }
    }
}

/// 死循环根因裁决结论。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopResolution {
    pub root_cause: LoopRootCause,
    /// 下一轮的具体换路径动作(作为局部重试的 hint 注入单元描述)。
    pub retry_path: String,
    pub retryable: bool,
    pub rationale: String,
}

impl LoopResolution {
    /// 裁决通道自身失败时的机械兜底(fail-open)。
    ///
    /// 归类规则只用诊断里已有的机械事实:循环里出现过工具 → `same_action`
    /// (最常见的「同一动作原地打转」);一轮工具都没调过 → `context_stuck`
    /// (纯文本自锁)。`retryable` 一律 `true`:**兜底不能替模型做不可重试
    /// 的判断** —— 那是唯一会造成「有救的单元被直接判死」的错误方向。
    pub fn mechanical_fallback(diag: &crate::agent::dead_loop::DeadLoopDiagnosis) -> Self {
        let (root_cause, retry_path) = if diag.tools.is_empty() {
            (
                LoopRootCause::ContextStuck,
                "上一轮未发起任何工具调用,模型在原地复述/重写同一段内容。\
                 下一轮请直接基于已有信息收口输出终答,不要重写计划;\
                 若确需新信息,只调用一次工具并立刻用它产生产物。"
                    .to_string(),
            )
        } else {
            (
                LoopRootCause::SameAction,
                format!(
                    "上一轮反复调用了同样的工具({}),下一轮**禁止**重复该调用:\
                     换一个此前没碰过的工具/路径/参数层级,或基于已获信息直接收口。",
                    diag.tools.join(" / ")
                ),
            )
        };
        Self {
            root_cause,
            retry_path,
            retryable: true,
            rationale: "Quality-Check 根因裁决通道解析失败,回落到机械兜底建议。".to_string(),
        }
    }

    /// 合并进常规 QC 结论:重试建议前置、retryable 取或。
    ///
    /// 「取或」而非「取与」的理由:常规 QC 只看文本,对「方向对但路径错」
    /// 常常给 `retryable=false`(保守),而本通道基于进程级诊断认为值得换路径
    /// 再试一次。两者不一致时,**选代价更小的一侧**(允许再试一轮),
    /// 避免一个有救的单元被单方面判死。
    pub fn merge_into(self, report: &mut QualityReport) {
        if !self.retry_path.trim().is_empty() {
            report.suggestion = format!(
                "[死循环根因 {}] {}\n[常规 QC] {}",
                self.root_cause.as_str(),
                self.retry_path.trim(),
                report.suggestion.trim()
            );
        }
        report.retryable = report.retryable || self.retryable;
        if report.evidence.trim().is_empty() {
            report.evidence = self.rationale;
        }
    }
}

/// 组装死循环根因裁决提示词(纯函数,便于单测)。
pub(super) fn build_dead_loop_prompt(
    unit_scope: &str,
    expected_output: &str,
    actual_output: &str,
    diag: &crate::agent::dead_loop::DeadLoopDiagnosis,
) -> String {
    format!(
        "【Quality-Check: 死循环根因裁决】\n\
         一个执行单元已被死循环守卫止损。你**不判产物对错**,只判两件事:\n\
         \x20 (1) 为什么会循环(根因归类);\n\
         \x20 (2) 下一轮该换哪条路(可执行的换路径动作)。\n\
         本单元职责: {unit_scope}\n\
         本单元期望输出: {expected_output}\n\
         实际输出: {actual}\n\
         \n\
         【死循环诊断(进程级事实,零幻觉)】\n{detail}\n\
         【归类判据】从下列固定枚举中选**一个**,不要自创:\n\
         \x20 - `same_action`:反复执行同一个动作/同一组动作,换个工具、参数或路径即可突破;\n\
         \x20 - `same_target`:目标本身不可达(页面/接口/权限/数据不存在),换动作无用;\n\
         \x20 - `context_stuck`:上下文里已有信息但模型在复述重写,压缩上下文/直接收口可解;\n\
         \x20 - `budget_starved`:方向正确但预算被无效动作烧光,砍步骤只做最关键的一件事。\n\
         【retryable 判据】根因是「路径选择错误」(same_action / context_stuck / budget_starved)\n\
         时给 true;根因是 same_target 且目标客观不可达时给 false(再试一轮只会重复打转)。\n\
         【retry_path 要求】必须是下一轮**具体执行什么**,禁止「换个方式试试」这类空话。\n\
         \n\
         只输出一个合法 JSON 对象:\n\
         {{\"root_cause\":\"same_action\",\"retry_path\":\"具体动作\",\"retryable\":true,\"rationale\":\"依据\"}}\n\
         root_cause 必须是上述四个枚举值之一。",
        actual = clip_qc_input(actual_output),
        detail = diag.render(),
    )
}

/// 解析死循环根因裁决输出(fail-open:解析失败返回 `None` 由调用方兜底)。
pub(super) fn parse_loop_resolution(text: &str) -> Option<LoopResolution> {
    let json_str = extract_json_block(text)
        .or_else(|| extract_standalone_json(text))?;
    let prepared = crate::agent::json_repair::repair_json(json_str);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&prepared) {
        let root_cause = LoopRootCause::parse(v.get("root_cause")?.as_str()?)?;
        let retry_path = v
            .get("retry_path")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if retry_path.is_empty() {
            return None;
        }
        return Some(LoopResolution {
            root_cause,
            retry_path,
            // retryable 缺省按根类推导:`same_target` 是唯一天然不可重试的一类。
            retryable: v
                .get("retryable")
                .and_then(|x| x.as_bool())
                .unwrap_or(root_cause != LoopRootCause::SameTarget),
            rationale: v
                .get("rationale")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .trim()
                .to_string(),
        });
    }
    None
}
