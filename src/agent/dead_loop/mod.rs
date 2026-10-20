//! 死循环守卫(LLM 交互级指纹检测):小循环 / 大循环 双形态识别 + 分级裁决。
//!
//! # 为什么在 [`crate::agent::loop_guard`] 之外再设一道
//!
//! `LoopGuard` 是**工具级**守卫,判据是「相邻两轮的 工具 + 参数 + 结果」,
//! 有三条结构性盲区:
//!
//! | 盲区 | 现象 | LoopGuard 为何抓不到 |
//! |------|------|---------------------|
//! | A 换序打转 | `Read(a)→Read(b)→Read(a)→Read(b)…` | `last_key` 只比相邻两轮,换动作即重置链 |
//! | B 换工具打转 | `Grep(X)→Bash(grep X)→Read(a)→Grep(X)` | 同上,链被反复打断 |
//! | C 上下文自锁 | 反复改写同一段输出/重交同一份 JSON | 没有工具调用就没有 `observe` |
//!
//! 三者的根因是:**死循环发生在「模型看到的输入状态 ↔ 模型产出的输出」这一层,
//! 而不在动作层**。本模块把判定键上移到 LLM 交互层,补齐全部三条盲区。
//!
//! # 指纹怎么算才有判别力(本模块最关键的两处设计)
//!
//! 1. **输入侧不能用「整个上下文」或「尾部 N 条消息」**:前者每轮必变,
//!    后者在循环里每轮整体错位 —— 两种朴素投影都会让重复检测彻底失效。
//!    本模块改为折叠**「模型已见信息集合」**(有序去重摘要集),语义是
//!    「`in_hash` 只在本轮上下文出现此前未见的新信息时才变化」。
//!    详见 [`fingerprint`] 模块文档与设计文档 §4。
//! 2. **长窗口判据不能用「动作重复率高」**:那会误杀编译修复场景
//!    (`Bash cargo check` 连跑 6 次修 6 个编译错,每次输出都不同,属健康工作)。
//!    本模块的低新颖度判据只看 `in_hash` 是否冻结,不看动作重复率。
//!
//! 设计见 `docs/LLM交互死循环指纹检测与质检裁决/01-设计与解决方案.md`。

pub(crate) mod detect;
pub(crate) mod fingerprint;
#[cfg(test)]
mod tests;

use serde_json::Value;

use crate::llm::ChatMessage;

pub use detect::{DeadLoopDiagnosis, LoopKind};

use detect::{
    detect_cycle, detect_low_novelty, detect_out_repeat, materialize, SHORT_PERIOD_MAX,
};
use fingerprint::FingerprintIndex;

/// 低新颖度窗口默认长度(环境变量 `LAEW_DEAD_LOOP_LONG_WINDOW` 可覆盖)。
const LONG_WINDOW_DEFAULT: usize = 12;

/// 一次 [`DeadLoopGuard::end_turn`] 的裁决。
///
/// 与 [`crate::agent::loop_guard::LoopVerdict`] 同构,保证两套守卫
/// 在 Agent 循环里的接线方式与升级阶梯完全一致。
#[derive(Debug, Clone)]
pub(crate) enum DeadLoopVerdict {
    /// 有进展。
    Progress,
    /// 软提醒:文案暂存守卫,下一轮经 `meta.runtime_tail` 注入消息流末尾
    /// (不污染 system 前缀,不破坏 prompt cache)。
    Nudge(String),
    /// 宽限轮强提醒:**立即**作为 user 消息推入上下文(保证送达 LLM)。
    Grace(String),
    /// 止损:终止本单元,诊断写入 `ExecutionTrace.dead_loop`。
    Abort(DeadLoopDiagnosis),
}

/// LLM 交互级死循环守卫。
///
/// 生命周期 = 一次 `run_session_body`(与 `LoopGuard` / `ExecutionTrace` 同域),
/// 全内存、不落库、不落盘,单元结束即释放。
#[derive(Debug)]
pub(crate) struct DeadLoopGuard {
    idx: FingerprintIndex,
    enabled: bool,
    long_window: usize,
    /// 已发过软提醒的循环链签名(签名 = `kind#period`)。
    nudged_sig: Option<String>,
    /// 宽限轮会话级一次性(对齐 `LoopGuard::abort_grace_used` 的 F9 教训)。
    grace_used: bool,
    /// 待下一轮注入的软提醒文案。
    pending_nudge: Option<String>,
    /// 最近一次命中的诊断(即便开关关闭也保留统计与诊断,可观测性不丢)。
    last_diag: Option<DeadLoopDiagnosis>,
    /// 本单元观测到的最大重复次数(供 trace / TUI / Debug 报告观测)。
    max_repeats: usize,
}

impl Default for DeadLoopGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl DeadLoopGuard {
    /// 按环境开关构造(默认开启,`LAEW_DEAD_LOOP=off` 关闭)。
    pub(crate) fn new() -> Self {
        Self::with_settings(dead_loop_enabled(), long_window())
    }

    /// 显式指定开关(单测用)。关闭后 [`end_turn`](Self::end_turn) 恒返回
    /// [`DeadLoopVerdict::Progress`],但检测照跑 —— `max_repeats` 与诊断仍更新。
    pub(crate) fn with_enabled(enabled: bool) -> Self {
        Self::with_settings(enabled, LONG_WINDOW_DEFAULT)
    }

    /// 显式指定开关 + 长循环窗口长度(单测用)。
    pub(crate) fn with_settings(enabled: bool, long_window: usize) -> Self {
        Self {
            idx: FingerprintIndex::new(),
            enabled,
            long_window: long_window.max(SHORT_PERIOD_MAX + 1),
            nudged_sig: None,
            grace_used: false,
            pending_nudge: None,
            last_diag: None,
            max_repeats: 0,
        }
    }

    /// 本单元观测到的最大重复次数。
    pub(crate) fn max_repeats(&self) -> usize {
        self.max_repeats
    }

    /// 取走待注入的软提醒文案(消费后清空)。
    pub(crate) fn take_nudge(&mut self) -> Option<String> {
        self.pending_nudge.take()
    }

    /// 最近一次命中的死循环诊断(供 `ExecutionTrace` / QC 取证)。
    pub(crate) fn diagnosis(&self) -> Option<&DeadLoopDiagnosis> {
        self.last_diag.as_ref()
    }

    /// 轮开始:累计上下文信息集合并算出本轮输入指纹,返回轮次下标。
    ///
    /// 必须在 `llm.complete(...)` **之前**调用。
    pub(crate) fn begin_turn(
        &mut self,
        agent_tag: &str,
        context: &[ChatMessage],
        forced_tool: Option<&str>,
        tool_names: &[String],
    ) -> usize {
        self.idx
            .begin_turn(agent_tag, context, forced_tool, tool_names)
    }

    /// 轮结束:回填输出指纹、跑检测、给出裁决。
    ///
    /// 必须在 `llm.complete(...)` **之后**、工具执行**之前**调用 ——
    /// 这样止损能拦住本轮全部工具调用,不产生孤儿 tool_use。
    pub(crate) fn end_turn(
        &mut self,
        iter: usize,
        text: &str,
        thinking: &str,
        calls: &[(String, Value)],
    ) -> DeadLoopVerdict {
        self.idx.end_turn(iter, text, thinking, calls);
        // 检测恒跑(开关只关裁决),保证 `LAEW_DEAD_LOOP=off` 时可观测性不丢。
        // 检测顺序即证据强度:输出逐字节重复(最短铁证) > 键序列周期扫描
        // (最精确的周期) > 长窗口低新颖度(最宽容,覆盖换措辞不换信息的形态)。
        let cand = detect_out_repeat(&self.idx)
            .or_else(|| detect_cycle(&self.idx))
            .or_else(|| detect_low_novelty(&self.idx, self.long_window));
        let Some(cand) = cand else {
            return DeadLoopVerdict::Progress;
        };
        let diag = materialize(&self.idx, &cand);
        self.max_repeats = self.max_repeats.max(cand.reps);
        self.last_diag = Some(diag.clone());
        if !self.enabled {
            tracing::warn!(
                kind = diag.kind.as_str(),
                "死循环诊断命中但守卫已关闭(LAEW_DEAD_LOOP=off),仅统计不裁决"
            );
            return DeadLoopVerdict::Progress;
        }

        if cand.kind.is_long() {
            // 大循环跳过软提醒直达宽限轮:能在长窗口上命中的单元,
            // 预算本就所剩无几,「礼貌提醒一轮」只是白烧最后一轮。
            if !self.grace_used {
                self.grace_used = true;
                tracing::warn!(
                    agent = "dead_loop",
                    kind = diag.kind.as_str(),
                    period = ?diag.period,
                    reps = diag.reps,
                    turns = diag.turns,
                    "死循环宽限轮:长循环/低新颖度形态,强提醒已排队,本轮结束后送达"
                );
                return DeadLoopVerdict::Grace(grace_text(&diag));
            }
            tracing::warn!(
                agent = "dead_loop",
                kind = diag.kind.as_str(),
                reps = diag.reps,
                "死循环止损终止:宽限轮已用,终止本单元"
            );
            return DeadLoopVerdict::Abort(diag);
        }

        // 小循环:软提醒 → 宽限轮 → 止损(同 LoopGuard 的三档阶梯)。
        let sig = cand.kind.signature(cand.period);
        if self.nudged_sig.as_deref() != Some(sig.as_str()) {
            self.nudged_sig = Some(sig);
            tracing::info!(
                agent = "dead_loop",
                kind = diag.kind.as_str(),
                period = ?diag.period,
                reps = diag.reps,
                "死循环软提醒:检测到重复轮次链(不终止)"
            );
            let text = nudge_text(&diag);
            self.pending_nudge = Some(text.clone());
            return DeadLoopVerdict::Nudge(text);
        }
        if !self.grace_used {
            self.grace_used = true;
            tracing::warn!(
                agent = "dead_loop",
                period = ?diag.period,
                reps = diag.reps,
                "死循环宽限轮:重复轮次链再次命中,强提醒已排队,本轮结束后送达"
            );
            return DeadLoopVerdict::Grace(grace_text(&diag));
        }
        tracing::warn!(
            agent = "dead_loop",
            period = ?diag.period,
            reps = diag.reps,
            "死循环止损终止:重复轮次链连续命中且宽限轮已用,终止本单元"
        );
        DeadLoopVerdict::Abort(diag)
    }
}

fn kind_cn(kind: LoopKind) -> &'static str {
    match kind {
        LoopKind::ShortCycle => "小循环(短周期)",
        LoopKind::LongCycle => "大循环(长周期)",
        LoopKind::LowNovelty => "大循环(低新颖度)",
    }
}

fn nudge_text(diag: &DeadLoopDiagnosis) -> String {
    format!(
        "【循环警告】检测到你与上一轮的**输入状态 + 输出**完全重复,形态 = {}。\
         再重复一次本轮将进入强制止损。立即改变策略:\
         (1) 不要重发同一段话 / 同一组工具调用,哪怕只是换个措辞;\
         (2) 换一个**信息源**(换工具、换路径、换 selector、换接口);\
         (3) 若已确认无法推进,直接收口并如实说明卡点。\
         诊断: {}",
        kind_cn(diag.kind),
        diag.headline(),
    )
}

fn grace_text(diag: &DeadLoopDiagnosis) -> String {
    format!(
        "[死循环止损宽限轮]\n\
         检测到你正在**原地打转**,形态 = {kind},周期 {period},已重复 {reps} 次,\
         参与轮次 {turns}。\n\
         判定依据:「模型已见信息集合」指纹冻结 —— 说明你最近这几轮**没有获得任何新信息**,\
         却在重复同一套动作/同一段输出。\n\
         循环内出现的工具: {tools}\n\
         诊断: {headline}\n\
         下一次再命中本形态,本单元将被强制终止并判为循环失败。立刻做以下之一:\n\
         \x20 (1) 换一个**此前没碰过**的信息源(换工具 / 换路径 / 换页面 / 换接口);\n\
         \x20 (2) 若已拿到足够信息,直接输出终答收口(完成情况 + 实际产出与期望的对应关系);\n\
         \x20 (3) 若确实无法推进,明确说明卡点与已尝试过的方法,不要继续重复。",
        kind = kind_cn(diag.kind),
        period = diag.period.map(|p| p.to_string()).unwrap_or_else(|| "无固定".into()),
        reps = diag.reps,
        turns = diag.turns,
        tools = if diag.tools.is_empty() {
            "(无)".to_string()
        } else {
            diag.tools.join(" / ")
        },
        headline = diag.headline(),
    )
}

/// 止损终止时的兜底叙事文本(交给 QC / 用户层)。
pub(crate) fn abort_fallback_text(diag: &DeadLoopDiagnosis) -> String {
    format!(
        "[LLM 交互死循环止损]\n\
         - 触发形态: {kind}\n\
         - 诊断摘要: {headline}\n\
         - 判定依据:对每轮 LLM 交互的输入状态指纹(「模型已见信息集合」折叠值)与\
           输出指纹(text+thinking+tool_calls)分别取 64 位哈希写入内存索引表,\
           对轮次键序列做尾部周期扫描 + 长窗口低新颖度检测\n\
         - 循环内工具: {tools}\n\
         - 输出取样: {sample}\n\
         请 Quality-Check 据此归类根因并给出换路径建议(不要重复同一动作)。",
        kind = kind_cn(diag.kind),
        headline = diag.headline(),
        tools = if diag.tools.is_empty() {
            "(无)".to_string()
        } else {
            diag.tools.join(" / ")
        },
        sample = diag.sample_out,
    )
}

/// 死循环守卫总开关(默认开启,`LAEW_DEAD_LOOP=off` 关闭)。
pub(crate) fn dead_loop_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        enabled_from(std::env::var("LAEW_DEAD_LOOP").unwrap_or_default())
    })
}

/// 开关取值解析(独立出来便于单测,惯例同 `loop_guard::react_guard_enabled_from`)。
pub(crate) fn enabled_from(raw: String) -> bool {
    !matches!(
        raw.trim().to_lowercase().as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 长循环低新颖度窗口长度(`LAEW_DEAD_LOOP_LONG_WINDOW`,默认 12)。
pub(crate) fn long_window() -> usize {
    long_window_from(std::env::var("LAEW_DEAD_LOOP_LONG_WINDOW").unwrap_or_default())
}

/// 窗口长度解析(独立出来便于单测)。非法值回落默认 12。
pub(crate) fn long_window_from(raw: String) -> usize {
    raw.trim()
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= SHORT_PERIOD_MAX + 1)
        .unwrap_or(LONG_WINDOW_DEFAULT)
}
