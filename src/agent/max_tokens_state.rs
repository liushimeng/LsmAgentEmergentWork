//! max_tokens 静默升级状态机(2026-09-09 第 09 轮,实现 L1037)。
//!
//! **动机**:Anthropic 客户端 `DEFAULT_MAX_TOKENS = 8192` 写死,对支持 64K 输出的
//! Sonnet/Opus 来说,稍长的工具结果汇总或代码 diff 就会被截断,触发自动截断续接
//! 骨架消耗续接次数才能拼齐。OpenAI 客户端根本不传 `max_tokens`,走 Provider 默认,
//! 长代码回复容易静默截断。claudecode `query.ts:1186` 的实践是「max_tokens 截断时
//! 自动翻倍,8K→16K→32K→64K,封顶 64K」,对模型而言是零成本修复。
//!
//! **设计**:
//! - 单会话级状态(每 `Agent::run_session` 调用创建一个新实例),`Arc<MaxTokensState>`
//!   持有。跨 LLM 调用维持,避免「下一轮又回 8K 又被截断」。
//! - 仅在 LLM 返回 `stop_reason = "max_tokens"` / `"length"` 时翻倍;`end_turn` /
//!   `stop` / `tool_use` 不触发,避免无意义的高 token 账单。
//! - 起始 8192,上限 65536,每次翻倍。
//! - 可观测:累计升级次数 + 历史 (old, new) 列表,写入 ExecutionTrace 供 QC / Debug。
//!
//! 设计见 `tmpPlan/2026-09-09_09-max-tokens静默升级与失败计数预警方案.md`。

use std::sync::Mutex;

/// max_tokens 起始值(对齐 Anthropic 既有 `DEFAULT_MAX_TOKENS = 8192`)。
pub const MAX_TOKENS_FLOOR: u32 = 8192;

/// max_tokens 上限(对齐 claudecode `MAX_OUTPUT_TOKENS = 65536` 实践)。
pub const MAX_TOKENS_CEIL: u32 = 65536;

/// 单 Provider 会话内的 max_tokens 升级状态机(线程安全)。
#[derive(Debug)]
pub struct MaxTokensState {
    inner: Mutex<MaxTokensInner>,
}

#[derive(Debug, Clone, Copy)]
struct MaxTokensInner {
    current: u32,
    upscalings: usize,
}

impl MaxTokensState {
    /// 新建状态机,起始 `MAX_TOKENS_FLOOR`(8K)。
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MaxTokensInner {
                current: MAX_TOKENS_FLOOR,
                upscalings: 0,
            }),
        }
    }

    /// 读当前值(供下一次 LLM 调用注入 `RequestMeta.max_tokens_override`)。
    pub fn current(&self) -> u32 {
        self.inner
            .lock()
            .expect("MaxTokensState mutex poisoned")
            .current
    }

    /// 标记「本轮被 max_tokens 截断」,翻倍一次(封顶 64K),返回升级后的值。
    /// 已经到上限时不再变化,返回当前值。
    pub fn note_truncated(&self) -> u32 {
        let mut g = self.inner.lock().expect("MaxTokensState mutex poisoned");
        if g.current < MAX_TOKENS_CEIL {
            let old = g.current;
            g.current = g.current.saturating_mul(2).min(MAX_TOKENS_CEIL);
            // 触顶时不再记升级次数(已封顶)
            if g.current != old {
                g.upscalings += 1;
            }
        }
        g.current
    }

    /// 累计升级次数(供 `ExecutionTrace.max_tokens_upscalings` 写入)。
    pub fn upscalings(&self) -> usize {
        self.inner
            .lock()
            .expect("MaxTokensState mutex poisoned")
            .upscalings
    }

    /// 升级历史 `(old, new)`,供 Debug Report / Agent-Memory 持久化调试。
    /// 当前实现是「累计次数 + 最终值」,更细粒度历史需要在 note_truncated 内
    /// 收集 old 值并 push 到 Vec,本轮先提供只读 API,后续按需扩展。
    pub fn history_snapshot(&self) -> Vec<(u32, u32)> {
        let g = self.inner.lock().expect("MaxTokensState mutex poisoned");
        if g.upscalings == 0 {
            Vec::new()
        } else {
            // 推算最近一次升级:从 FLOOR 经 upscalings 次翻倍
            let mut v = MAX_TOKENS_FLOOR;
            let mut out = Vec::with_capacity(g.upscalings);
            for _ in 0..g.upscalings {
                let new_v = v.saturating_mul(2).min(MAX_TOKENS_CEIL);
                out.push((v, new_v));
                v = new_v;
                if v >= MAX_TOKENS_CEIL {
                    break;
                }
            }
            out
        }
    }
}

impl Default for MaxTokensState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_at_floor() {
        let s = MaxTokensState::new();
        assert_eq!(s.current(), MAX_TOKENS_FLOOR);
        assert_eq!(s.upscalings(), 0);
        assert!(s.history_snapshot().is_empty());
    }

    #[test]
    fn first_truncation_doubles_to_16k() {
        let s = MaxTokensState::new();
        let new_v = s.note_truncated();
        assert_eq!(new_v, 16384);
        assert_eq!(s.current(), 16384);
        assert_eq!(s.upscalings(), 1);
        let hist = s.history_snapshot();
        assert_eq!(hist, vec![(8192, 16384)]);
    }

    #[test]
    fn multiple_truncations_double_chain() {
        let s = MaxTokensState::new();
        s.note_truncated(); // 8K → 16K
        s.note_truncated(); // 16K → 32K
        s.note_truncated(); // 32K → 64K
        assert_eq!(s.current(), 65536);
        assert_eq!(s.upscalings(), 3);
        let hist = s.history_snapshot();
        assert_eq!(hist, vec![(8192, 16384), (16384, 32768), (32768, 65536)]);
    }

    #[test]
    fn caps_at_64k_no_further_upscale() {
        let s = MaxTokensState::new();
        // 触发 10 次截断(远超封顶所需次数)
        for _ in 0..10 {
            s.note_truncated();
        }
        assert_eq!(s.current(), MAX_TOKENS_CEIL);
        // upscalings 只能升 3 次(8K→16K→32K→64K),之后不再变化
        assert_eq!(s.upscalings(), 3);
    }

    #[test]
    fn successful_completion_does_not_change_state() {
        // 模拟正常 end_turn 不调用 note_truncated,状态保持
        let s = MaxTokensState::new();
        // 不调用 note_truncated
        assert_eq!(s.current(), MAX_TOKENS_FLOOR);
        assert_eq!(s.upscalings(), 0);
    }

    #[test]
    fn note_truncated_is_idempotent_at_ceiling() {
        let s = MaxTokensState::new();
        for _ in 0..3 {
            s.note_truncated();
        }
        assert_eq!(s.current(), MAX_TOKENS_CEIL);
        let before = s.upscalings();
        // 已经在 64K 再截断:返回 64K,但不再记升级次数
        assert_eq!(s.note_truncated(), MAX_TOKENS_CEIL);
        assert_eq!(s.upscalings(), before);
    }

    #[test]
    fn thread_safe_concurrent_truncation() {
        // 8 个线程同时调用 note_truncated,upscalings 累计必须 = 3(8K→64K 三次)
        use std::sync::Arc;
        use std::thread;
        let s = Arc::new(MaxTokensState::new());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let s2 = Arc::clone(&s);
                thread::spawn(move || {
                    s2.note_truncated();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(s.current(), MAX_TOKENS_CEIL);
        assert_eq!(s.upscalings(), 3);
    }
}
