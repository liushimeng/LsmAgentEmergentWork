//! 死循环判定:小循环(短周期)/ 大循环(长周期 + 低新颖度)+ 诊断结构。
//!
//! 见 `docs/LLM交互死循环指纹检测与质检裁决/01-设计与解决方案.md` §5~§6。

use serde::{Deserialize, Serialize};

use super::fingerprint::{FingerprintIndex, LlmTurnFingerprint};

/// 短周期上界:周期 ≤ 该值算「小循环」,> 算「大循环」。
pub(crate) const SHORT_PERIOD_MAX: usize = 4;
/// 周期 1 的最少重复次数。
const REPS_MIN_PERIOD1: usize = 2;
/// 短周期(2~4)的最少重复次数。
const REPS_MIN_SHORT: usize = 3;
/// 长周期(≥5)的最少重复次数:两整圈已是铁证(长周期跑满三圈必然超预算)。
const REPS_MIN_LONG: usize = 2;
/// 低新颖度窗口最少「与上一轮输入指纹相同」的轮数。
const LOW_NOVELTY_MIN_FROZEN: usize = 6;

/// 死循环形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopKind {
    /// 小循环:2~4 轮一圈(period=1 时为硬自锁:输入状态与输出逐字节相同)。
    ShortCycle,
    /// 大循环:5 轮以上一圈的长周期打转。
    LongCycle,
    /// 大循环第二形态:上下文零新增信息,模型在低新颖度窗口里换措辞重复。
    LowNovelty,
}

impl LoopKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LoopKind::ShortCycle => "short_cycle",
            LoopKind::LongCycle => "long_cycle",
            LoopKind::LowNovelty => "low_novelty",
        }
    }

    /// 大循环跳过软提醒直达宽限轮(预算所剩无几,提醒一轮只是白烧)。
    pub(crate) fn is_long(self) -> bool {
        matches!(self, LoopKind::LongCycle | LoopKind::LowNovelty)
    }

    /// 循环链签名:换签名后软提醒档位重置。
    pub(crate) fn signature(self, period: Option<usize>) -> String {
        format!("{}#{}", self.as_str(), period.unwrap_or(0))
    }
}

/// 死循环诊断:写进 `ExecutionTrace`,供 QC 裁决与 Debug 报告取证。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLoopDiagnosis {
    pub kind: LoopKind,
    /// 最小周期(`LowNovelty` 为 `None`)。
    pub period: Option<usize>,
    /// 判定时的重复次数。
    pub reps: usize,
    /// 参与判定的总轮数。
    pub turns: usize,
    /// 窗口内不同 `in_hash` 个数。
    pub distinct_inputs: usize,
    /// 全会话不同 `out_hash` 个数。
    pub distinct_outputs: usize,
    /// 构成循环的轮次下标。
    pub cycle_turns: Vec<usize>,
    /// 输入预览(模型看到的状态取样)。
    pub sample_in: String,
    /// 输出预览(模型产出的取样)。
    pub sample_out: String,
    /// 循环中反复出现的工具。
    pub tools: Vec<String>,
}

impl DeadLoopDiagnosis {
    /// 一行摘要(写进 `failure_signals` / TUI / 运行日志)。
    pub fn headline(&self) -> String {
        match self.period {
            Some(p) => format!(
                "{}:p={},reps={},turns={},distinct_in={},distinct_out={}",
                self.kind.as_str(),
                p,
                self.reps,
                self.turns,
                self.distinct_inputs,
                self.distinct_outputs
            ),
            None => format!(
                "{}:distinct_in={}/{},frozen={},turns={}",
                self.kind.as_str(),
                self.distinct_inputs,
                self.turns,
                self.reps,
                self.turns
            ),
        }
    }

    /// 供 QC 提示词与裁决通道阅读的完整文本。
    pub fn render(&self) -> String {
        let period = match self.period {
            Some(p) => format!("最小周期 = {p} 轮"),
            None => "无固定周期".to_string(),
        };
        let tools = if self.tools.is_empty() {
            "(无工具调用,纯文本自锁)".to_string()
        } else {
            self.tools.join(" / ")
        };
        format!(
            "- 形态 = {}({period});重复 {reps} 次;参与轮次 {turns}\n\
             - 窗口内不同输入状态 = {di};全会话不同输出 = {do}\n\
             - 循环内出现的工具: {tools}\n\
             - 输入取样: {si}\n\
             - 输出取样: {so}",
            self.kind.as_str(),
            reps = self.reps,
            turns = self.turns,
            di = self.distinct_inputs,
            do = self.distinct_outputs,
            si = self.sample_in,
            so = self.sample_out,
        )
    }
}

/// 键序列尾部最长的「p-周期后缀」。
///
/// 返回 `(period, reps)`:`period` 为最小周期,`reps` 为该周期后缀里
/// 单元出现的次数(后缀长度 = `period * reps`)。
///
/// **为什么不能用「整串最小周期」**(KMP `n - pi[n-1]` 那一套):整串最小
/// 周期要求**从头到尾**都满足 `s[i] == s[i-p]`,一旦序列前面有一段瞬态
/// (首轮状态天然与后续不同,如 `(H0,o), (H1,o), (H1,o), …`)就恒判不出周期,
/// 而那恰恰是最典型的硬自锁形态。这里改为**从尾部反扫**,只要求尾部稳态
/// 部分构成循环 —— 瞬态前缀不再遮蔽循环。
///
/// 反扫方式:从末尾往回,只要 `keys[i] == keys[i-p]` 就继续,统计连续步数
/// `streak`,则 p-周期后缀长度 = `p + streak`。代价 O(n²/2)(n ≤ 64 时
/// 约 2 千次比较),可忽略。
pub(crate) fn min_periodic_suffix(keys: &[(u64, u64)]) -> Option<(usize, usize)> {
    let n = keys.len();
    let mut best: Option<(usize, usize)> = None;
    for p in 1..=n / 2 {
        let mut streak = 0usize;
        let mut i = n;
        while i > p && keys[i - 1] == keys[i - 1 - p] {
            streak += 1;
            i -= 1;
        }
        let suffix_len = p + streak;
        let reps = suffix_len / p;
        if reps < reps_threshold(p) {
            continue;
        }
        best = Some((p, reps));
        break; // 自小周期起扫描,首个满足门槛者即最小周期
    }
    best
}

/// 重复次数门槛:周期 1 两次即铁证(上下文在增长而输出逐字节相同);
/// 短周期需三圈(两圈可能只是正常的二次确认);长周期两圈即可。
pub(crate) fn reps_threshold(period: usize) -> usize {
    if period == 1 {
        REPS_MIN_PERIOD1
    } else if period <= SHORT_PERIOD_MAX {
        REPS_MIN_SHORT
    } else {
        REPS_MIN_LONG
    }
}

/// 汇总循环内的工具名(去重 + 排序,便于对账)。
fn cycle_tools(steps: &[LlmTurnFingerprint], turns: &[usize]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for t in turns {
        if let Some(s) = steps.get(*t) {
            for n in &s.tools {
                if !names.contains(n) {
                    names.push(n.clone());
                }
            }
        }
    }
    names.sort();
    names
}

/// 候选诊断(尚未决定是否成立,由 `mod.rs` 的守卫裁决)。
pub(crate) struct Candidate {
    pub kind: LoopKind,
    pub period: Option<usize>,
    pub reps: usize,
    /// 参与判定的轮数(周期判定的全序列长度 / 低新颖度的窗口长度)。
    pub turns: usize,
    pub distinct_inputs: usize,
    pub cycle_turns: Vec<usize>,
}

/// 周期检测:小循环 / 大循环。
pub(crate) fn detect_cycle(idx: &FingerprintIndex) -> Option<Candidate> {
    let steps = &idx.steps;
    if steps.len() < 2 {
        return None;
    }
    let keys: Vec<(u64, u64)> = steps.iter().map(LlmTurnFingerprint::key).collect();
    let (period, reps) = min_periodic_suffix(&keys)?;
    let kind = if period <= SHORT_PERIOD_MAX {
        LoopKind::ShortCycle
    } else {
        LoopKind::LongCycle
    };
    let start = steps.len() - period * reps;
    let cycle_turns: Vec<usize> = (start..steps.len()).collect();
    let distinct_inputs = {
        let mut set = std::collections::HashSet::new();
        for s in steps {
            set.insert(s.in_hash);
        }
        set.len()
    };
    Some(Candidate {
        kind,
        period: Some(period),
        reps,
        turns: steps.len(),
        distinct_inputs,
        cycle_turns,
    })
}

/// 输出逐字节重复检测(硬自锁的最短路径)。
///
/// 键周期扫描要求 **(输入状态, 输出) 二元组** 整体重复;但「上下文还在长、
/// 模型却一字不差地又产出同一个响应」本身就已经是铁证 —— 上下文给了它**更多**
/// 信息,它却交回同一份答案。键扫描要等到信息集合冻结(多轮之后)才响,这里
/// 提前一轮抓到。
///
/// 间隔约束:上一次相同 `out_hash` 出现必须在 [`SHORT_PERIOD_MAX`] 轮之内。
/// 相隔很远(中间夹着大量不同输出)时不算 —— 那更像「回到某个先前状态」,
/// 交给周期扫描或低新颖度判据处理,不在这里草率定性。
pub(crate) fn detect_out_repeat(idx: &FingerprintIndex) -> Option<Candidate> {
    let last = idx.steps.last()?;
    if last.out_hash == 0 {
        return None; // 输出指纹尚未回填(不应发生,防御性跳过)
    }
    let (prev, cur) = idx.prev_out_round(last.round)?;
    if cur - prev != 1 {
        return None;
    }
    let distinct_inputs = {
        let mut set = std::collections::HashSet::new();
        for s in &idx.steps {
            set.insert(s.in_hash);
        }
        set.len()
    };
    Some(Candidate {
        kind: LoopKind::ShortCycle,
        period: Some(1),
        reps: 2,
        turns: idx.steps.len(),
        distinct_inputs,
        cycle_turns: vec![prev, cur],
    })
}

/// 低新颖度检测:大循环第二形态。
///
/// 判据的安全来自 [`super::fingerprint`] 的构造 —— `in_hash` 相同 ⟺ 本轮上下文
/// 零新增消息摘要。健康轮次每轮必然新增 assistant 消息与工具结果,
/// `distinct_in` 恒等于窗口长度;只有真正打转时输入指纹才会冻结。
///
/// **刻意不用「action_hash 重复率高」做长窗口判据**:那样会误杀编译修复场景
/// (`Bash cargo check` 连跑 6 次修 6 个编译错,每次输出都不同,属健康工作)。
pub(crate) fn detect_low_novelty(idx: &FingerprintIndex, long_window: usize) -> Option<Candidate> {
    let (distinct, frozen, window) = idx.distinct_inputs_in_window(long_window);
    if window < long_window {
        return None;
    }
    let distinct_cap = std::cmp::max(2, window / 3);
    if distinct > distinct_cap || frozen < LOW_NOVELTY_MIN_FROZEN {
        return None;
    }
    let start = idx.steps.len().saturating_sub(window);
    Some(Candidate {
        kind: LoopKind::LowNovelty,
        period: None,
        reps: frozen,
        turns: window,
        distinct_inputs: distinct,
        cycle_turns: (start..idx.steps.len()).collect(),
    })
}

/// 候选 → 诊断(取样从循环窗口首轮取,便于人工对账)。
pub(crate) fn materialize(idx: &FingerprintIndex, cand: &Candidate) -> DeadLoopDiagnosis {
    let anchor = cand.cycle_turns.first().copied().unwrap_or(0);
    let (sample_in, sample_out, tools) = match idx.steps.get(anchor) {
        Some(s) => (s.in_preview.clone(), s.out_preview.clone(), cycle_tools(&idx.steps, &cand.cycle_turns)),
        None => (String::new(), String::new(), Vec::new()),
    };
    DeadLoopDiagnosis {
        kind: cand.kind,
        period: cand.period,
        reps: cand.reps,
        turns: cand.turns,
        distinct_inputs: cand.distinct_inputs,
        distinct_outputs: idx.distinct_outputs(),
        cycle_turns: cand.cycle_turns.clone(),
        sample_in,
        sample_out,
        tools,
    }
}
