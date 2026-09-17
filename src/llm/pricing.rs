//! 模型价格表与成本估算(D8 实时成本,2026-09-17 第 76 轮)。
//!
//! 设计对齐 claudecode `src/utils/modelCost.ts`(写死 tier 价格表 + 按 Mtok 计价
//! + cache 读取折扣/写入溢价),参考
//! `docs/Agent源码调研/专题/专题-第三轮-成本控制与Token统计深度分析.md`。
//!
//! 原则:
//! - **零网络零新 crate**:价格为内置参考价(2026-09,官方刊例),仅用于估算,
//!   不保证与账单一致;Provider 实际定价以各厂商为准。
//! - **未知模型 → None**(不估价):本地模型(Ollama)按 $0 误估、或私有网关
//!   套高价表,都比"不估价"更误导。调用方据此显示「无内置参考价」。
//! - 匹配规则:模型名 lowercase 后做 contains 匹配,**最长 pattern 优先**
//!   (`gpt-4o-mini` 必须先于 `gpt-4o` 命中)。

use crate::llm::Usage;

/// 单模型价格,单位:USD / 1M tokens。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    /// 普通输入 token 单价。
    pub input: f64,
    /// 输出 token 单价。
    pub output: f64,
    /// 缓存读取单价(命中 prompt cache 的输入侧,通常远低于 input)。
    pub cache_read: f64,
    /// 缓存写入单价(Anthropic cache_creation;OpenAI 不区分,取 input)。
    pub cache_write: f64,
}

impl ModelPrice {
    const fn new(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Self {
        Self {
            input,
            output,
            cache_read,
            cache_write,
        }
    }
}

/// 内置参考价格表(2026-09 刊例价,仅估算)。**按 pattern 长度降序排列**,
/// 查找时线性扫描取首个 contains 命中,保证最具体 pattern 优先。
///
/// cache 价格规则:
/// - Anthropic 系:cache_write = 1.25 × input(5m  TTL),cache_read = 0.1 × input;
/// - OpenAI 系:cache_write = input(不区分写入),cache_read = 官方 cached 价;
/// - DeepSeek:cache_read = 命中价,cache_write = input(未命中即普通输入价)。
static PRICE_TABLE: &[(&str, ModelPrice)] = &[
    // ---- Claude(Anthropic)----
    ("claude-opus-4-5", ModelPrice::new(5.0, 25.0, 0.5, 6.25)),
    ("claude-opus-4", ModelPrice::new(15.0, 75.0, 1.5, 18.75)),
    ("claude-haiku-4-5", ModelPrice::new(1.0, 5.0, 0.1, 1.25)),
    ("claude-haiku-3-5", ModelPrice::new(0.8, 4.0, 0.08, 1.0)),
    // 旧命名变体(claude-3-5-haiku-20241022 词序不同,需独立 pattern)
    ("claude-3-5-haiku", ModelPrice::new(0.8, 4.0, 0.08, 1.0)),
    // sonnet 全系(3.5/3.7/4/4.5)同档 3/15;旧命名 claude-3-5-sonnet 亦含 "sonnet"
    ("sonnet", ModelPrice::new(3.0, 15.0, 0.3, 3.75)),
    // ---- OpenAI ----
    ("gpt-4o-mini", ModelPrice::new(0.15, 0.6, 0.075, 0.15)),
    ("gpt-5-mini", ModelPrice::new(0.25, 2.0, 0.025, 0.25)),
    ("gpt-4o", ModelPrice::new(2.5, 10.0, 1.25, 2.5)),
    ("gpt-5", ModelPrice::new(1.25, 10.0, 0.125, 1.25)),
    ("o4-mini", ModelPrice::new(1.1, 4.4, 0.275, 1.1)),
    ("o3", ModelPrice::new(2.0, 8.0, 0.5, 2.0)),
    // ---- DeepSeek ----
    ("deepseek-reasoner", ModelPrice::new(0.55, 2.19, 0.14, 0.55)),
    ("deepseek-chat", ModelPrice::new(0.27, 1.1, 0.07, 0.27)),
];

/// 按模型名查内置参考价;未收录返回 None(调用方应显示「无内置参考价」而非 $0)。
pub fn lookup_price(model_name: &str) -> Option<ModelPrice> {
    let m = model_name.to_lowercase();
    // 表已按 pattern 长度降序;线性扫描首个 contains 命中即最长匹配。
    // 防御:即使未来有人乱序加行,也用显式最长匹配兜底。
    let mut best: Option<(&str, ModelPrice)> = None;
    for (pat, price) in PRICE_TABLE {
        if m.contains(pat) {
            match best {
                Some((bp, _)) if bp.len() >= pat.len() => {}
                _ => best = Some((pat, *price)),
            }
        }
    }
    best.map(|(_, p)| p)
}

/// 成本四分量分解(供 `/cost` 面板展示)。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CostBreakdown {
    pub input_usd: f64,
    pub output_usd: f64,
    pub cache_read_usd: f64,
    pub cache_write_usd: f64,
}

impl CostBreakdown {
    pub fn total(&self) -> f64 {
        self.input_usd + self.output_usd + self.cache_read_usd + self.cache_write_usd
    }
}

/// 估算一次用量的成本(USD);模型无内置价时返回 None。
pub fn estimate_cost_usd(model_name: &str, usage: &Usage) -> Option<f64> {
    cost_breakdown(model_name, usage).map(|b| b.total())
}

/// 估算成本分解;模型无内置价时返回 None。
pub fn cost_breakdown(model_name: &str, usage: &Usage) -> Option<CostBreakdown> {
    let p = lookup_price(model_name)?;
    const M: f64 = 1_000_000.0;
    Some(CostBreakdown {
        input_usd: usage.input_tokens as f64 / M * p.input,
        output_usd: usage.output_tokens as f64 / M * p.output,
        cache_read_usd: usage.cache_read_input_tokens as f64 / M * p.cache_read,
        cache_write_usd: usage.cache_creation_input_tokens as f64 / M * p.cache_write,
    })
}

/// USD 金额自适应精度格式化:
/// ≥ $1 保留 2 位;≥ $0.01 保留 4 位;更小保留 6 位(避免微额显示成 $0.00)。
pub fn format_usd(cost: f64) -> String {
    if cost >= 1.0 {
        format!("${cost:.2}")
    } else if cost >= 0.01 {
        format!("${cost:.4}")
    } else if cost > 0.0 {
        format!("${cost:.6}")
    } else {
        "$0.00".to_string()
    }
}

/// 缓存命中率(cache_read / 输入侧总量);输入侧为 0 时返回 None。
pub fn cache_hit_rate(usage: &Usage) -> Option<f64> {
    let total = usage.input_tokens as f64
        + usage.cache_read_input_tokens as f64
        + usage.cache_creation_input_tokens as f64;
    if total <= 0.0 {
        None
    } else {
        Some(usage.cache_read_input_tokens as f64 / total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32, read: u32, write: u32) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: read,
            cache_creation_input_tokens: write,
        }
    }

    #[test]
    fn lookup_exact_and_case_insensitive() {
        assert!(lookup_price("gpt-4o").is_some());
        assert!(lookup_price("GPT-4o").is_some());
        assert!(lookup_price("claude-opus-4-5").is_some());
    }

    #[test]
    fn lookup_longest_pattern_wins() {
        // gpt-4o-mini 必须命中 0.15/0.6 档,而不是 gpt-4o 的 2.5/10 档
        let p = lookup_price("gpt-4o-mini-2024-07-18").unwrap();
        assert_eq!(p.input, 0.15);
        let p5 = lookup_price("gpt-5-mini").unwrap();
        assert_eq!(p5.input, 0.25);
    }

    #[test]
    fn lookup_anthropic_dated_suffix() {
        let p = lookup_price("claude-sonnet-4-5-20250929").unwrap();
        assert_eq!(p.input, 3.0);
        // 旧命名词序变体:claude-3-5-haiku-20241022
        let p = lookup_price("claude-3-5-haiku-20241022").unwrap();
        assert_eq!(p.input, 0.8);
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup_price("claude-mock").is_none());
        assert!(lookup_price("llama3.1-local").is_none());
        assert!(lookup_price("").is_none());
    }

    #[test]
    fn estimate_four_components() {
        // sonnet: input 3 / output 15 / read 0.3 / write 3.75 per Mtok
        let u = usage(1_000_000, 1_000_000, 1_000_000, 1_000_000);
        let c = estimate_cost_usd("claude-sonnet-4-5", &u).unwrap();
        assert!((c - (3.0 + 15.0 + 0.3 + 3.75)).abs() < 1e-9);
    }

    #[test]
    fn estimate_unknown_model_none() {
        assert!(estimate_cost_usd("claude-mock", &usage(100, 100, 0, 0)).is_none());
    }

    #[test]
    fn estimate_zero_usage_is_zero() {
        let c = estimate_cost_usd("gpt-4o", &Usage::default()).unwrap();
        assert_eq!(c, 0.0);
    }

    #[test]
    fn breakdown_totals_match_estimate() {
        let u = usage(123_456, 7_890, 50_000, 20_000);
        let b = cost_breakdown("deepseek-chat", &u).unwrap();
        let t = estimate_cost_usd("deepseek-chat", &u).unwrap();
        assert!((b.total() - t).abs() < 1e-12);
        assert!(b.input_usd > 0.0 && b.output_usd > 0.0);
        assert!(b.cache_read_usd > 0.0 && b.cache_write_usd > 0.0);
    }

    #[test]
    fn format_usd_adaptive_precision() {
        assert_eq!(format_usd(12.345), "$12.35");
        assert_eq!(format_usd(1.0), "$1.00");
        assert_eq!(format_usd(0.12345), "$0.1235");
        assert_eq!(format_usd(0.000123), "$0.000123");
        assert_eq!(format_usd(0.0), "$0.00");
    }

    #[test]
    fn cache_hit_rate_basic() {
        // read 50 / (input 100 + read 50 + write 50) = 25%
        let r = cache_hit_rate(&usage(100, 0, 50, 50)).unwrap();
        assert!((r - 0.25).abs() < 1e-9);
        assert!(cache_hit_rate(&Usage::default()).is_none());
    }

    #[test]
    fn deepseek_cache_hit_price() {
        // deepseek-chat: 命中价 0.07/M,远低于普通输入 0.27/M
        let p = lookup_price("deepseek-chat").unwrap();
        assert!(p.cache_read < p.input);
    }
}
