//! 上下文溢出自动检测与三级恢复(reactive,第 06 轮)。
//!
//! 与 proactive 的 Compact Agent(80% 估算阈值)互补:估算(字符/4 + 10%)对
//! CJK 等场景可能低估 2-3 倍,真实溢出(Provider 返回 `prompt is too long` /
//! `context_length_exceeded` 类 400 错误)时由本模块兜底:
//!
//! - **Level 1 排水**:`drain_tool_results` 原地截短超长 tool_result(结构不变,
//!   tool_use/tool_result 配对天然保持)后重试;
//! - **Level 2 折叠**:`fold_history` 把非保护段历史合并为一条压缩摘要消息
//!   (复用 Compact 的渲染 + 硬截断 + 保护标记)后重试;
//! - **Level 3 暴露**:两轮无效则原错误上抛,进入既有 Yolo 失败回流。
//!
//! 恢复手段全部本地化(不调 LLM)——溢出场景下再发摘要请求可能继续溢出,
//! 本地排水/折叠是确定性手段,与 compact.rs「LLM 失败降级硬截断」哲学一致。
//!
//! 知识库出处:第十六轮 claudecode §2.2「prompt-too-long 三级恢复」(L1038)+
//! 第十六轮 pi §8「20+ provider 溢出正则 + NON_OVERFLOW 排除集」(L1044)。
//! 方案见 `tmpPlan/2026-09-09_06-上下文溢出自动检测与三级恢复方案.md`。

use crate::agent::compact::{self, COMPACT_MARKER_END, COMPACT_MARKER_START};
use crate::error::AgentError;
use crate::llm::{ChatMessage, ContentBlock, Role};
use crate::session::Session;

/// 溢出错误特征(provider 错误消息子串,统一小写匹配)。
///
/// 覆盖 Anthropic / OpenAI 及主流网关 / 兼容层(OpenRouter / vLLM / Gemini …)
/// 的常见变体,对齐 pi `ai/utils/overflow.ts` 的正则集。
const OVERFLOW_PATTERNS: &[&str] = &[
    // Anthropic: "prompt is too long: 54321 tokens > 200000 maximum"
    "prompt is too long",
    "prompt too long",
    "prompt_too_long",
    // OpenAI: code=context_length_exceeded / "This model's maximum context length is ..."
    "context_length_exceeded",
    "context length exceeded",
    "maximum context length",
    // 通用 context window 表述(网关 / vLLM / OpenRouter)
    "exceeds the context window",
    "exceed the context window",
    "exceeded the context window",
    "context window exceeded",
    "beyond the context window",
    // input / token 表述(Gemini / Mistral / 兼容层)
    "input is too long",
    "input too long",
    "input length exceeds",
    "too many input tokens",
    "input tokens exceed",
    "too many tokens",
    // request_too_large 错误码形态(配合状态码门槛 400/413,不误伤普通 payload 过大)
    "request_too_large",
    // Anthropic 附带建议文案("please reduce the length of ...")
    "reduce the length",
];

/// 非溢出排除集(优先级高于 OVERFLOW_PATTERNS,对齐 pi `NON_OVERFLOW_PATTERNS`)。
///
/// 限流 / 配额 / 计费 / 鉴权类错误可能同时含 "too many tokens" 字样
/// (如 token-per-minute 限流),但压缩上下文对它们毫无帮助,必须排除。
const NON_OVERFLOW_PATTERNS: &[&str] = &[
    "rate limit",
    "rate_limit",
    "ratelimit",
    "throttl", // throttle / throttled / throttling
    "quota",
    "billing",
    "insufficient",
    "credit",
    "api key",
    "api_key",
    "unauthorized",
    "authentication",
    "permission denied",
];

/// 判断一个 LLM 错误是否为「上下文溢出」。
///
/// - 只认 `LlmHttp{400|413}` / `LlmStream` / `Llm` 三种载体
///   (500/429 等交给重试层与熔断器,本地折叠帮不上忙);
/// - NON_OVERFLOW 排除集优先命中 → 一定不是溢出;
/// - 其余按 OVERFLOW_PATTERNS 子串匹配(大小写不敏感)。
pub fn is_context_overflow(err: &AgentError) -> bool {
    let (status, message) = match err {
        AgentError::LlmHttp { status, message, .. } => (Some(*status), message.as_str()),
        // 部分网关以 200 + SSE error 事件或纯文本报溢出,无状态码可依
        AgentError::LlmStream { message, .. } => (None, message.as_str()),
        AgentError::Llm(m) => (None, m.as_str()),
        _ => return false,
    };
    if !matches!(status, None | Some(400) | Some(413)) {
        return false;
    }
    let lower = message.to_lowercase();
    if NON_OVERFLOW_PATTERNS.iter().any(|p| lower.contains(p)) {
        return false;
    }
    OVERFLOW_PATTERNS.iter().any(|p| lower.contains(p))
}

/// 单个工具结果排水门槛:超过该字符数才截短(小结果不动,避免无谓噪声)。
pub const DRAIN_MIN_CHARS: usize = 4_000;
/// 排水保留:头部字符数。
pub const DRAIN_KEEP_HEAD: usize = 2_000;
/// 排水保留:尾部字符数。
pub const DRAIN_KEEP_TAIL: usize = 1_000;

/// Level 1 排水:原地截短全部超长 tool_result(保头 + 保尾 + 省略标记)。
///
/// 消息结构不变(tool_use/tool_result 配对天然保持,协议一致性零风险),
/// 返回截短个数;0 表示没有可排水内容(调用方应落到 Level 2)。
/// 幂等:截短后约 `DRAIN_KEEP_HEAD + DRAIN_KEEP_TAIL` + 标记 < 门槛,
/// 二次调用返回 0。
pub fn drain_tool_results(context: &mut [ChatMessage]) -> usize {
    let mut drained = 0;
    for m in context.iter_mut() {
        for block in m.content.iter_mut() {
            let ContentBlock::ToolResult { content, .. } = block else {
                continue;
            };
            let n = content.chars().count();
            if n <= DRAIN_MIN_CHARS {
                continue;
            }
            let head: String = content.chars().take(DRAIN_KEEP_HEAD).collect();
            let tail: String = content.chars().skip(n - DRAIN_KEEP_TAIL).collect();
            *content = format!(
                "{head}\n…[overflow-drain 原结果 {n} 字符已截短,省略 {} 字符]…\n{tail}",
                n - DRAIN_KEEP_HEAD - DRAIN_KEEP_TAIL,
            );
            drained += 1;
        }
    }
    drained
}

/// Level 2 折叠:把「非保护段 + 尾部保留之外」的历史消息合并为一条
/// `<<<LAEW:COMPACTED_CONTEXT>>>` 压缩摘要消息(本地硬截断,不调 LLM)。
///
/// 返回折叠的消息条数;`None` 表示无折叠空间(候选为空 / 折叠段太小),
/// 调用方应上抛原错误(Level 3 暴露)。
///
/// 边界守卫与幂等:
/// - 尾部保留与 Compact Agent 一致(最近 `KEEP_RECENT_MESSAGES` 条);
/// - **配对守卫**:若尾部边界落在 tool_result 序列中间,把边界右移到 Tool
///   消息结束,避免折叠掉 assistant.tool_use 而留下孤儿 tool_result
///   (Anthropic/OpenAI 均会再报 400);
/// - 保护段(项目上下文 / 历史摘要 / 已压缩摘要)永不折叠;
/// - 折叠产物自带 `COMPACT_MARKER_START` → 本身即保护段,天然幂等。
pub fn fold_history(session: &mut Session) -> Option<usize> {
    let len = session.context().len();
    // 尾部保护:最近 KEEP_RECENT_MESSAGES 条不折叠
    let mut tail_start = len.saturating_sub(compact::KEEP_RECENT_MESSAGES);
    // 配对守卫:边界消息若为 Tool(其 assistant.tool_use 在折叠段内),
    // 一并纳入折叠段直到非 Tool 为止
    while tail_start < len && session.context()[tail_start].role == Role::Tool {
        tail_start += 1;
    }

    // 折叠候选:[0, tail_start) 内的非保护消息
    let compact_idx: Vec<usize> = (0..tail_start)
        .filter(|&i| !compact::is_protected(&session.context()[i]))
        .collect();
    if compact_idx.is_empty() {
        return None;
    }
    let source: Vec<ChatMessage> = compact_idx
        .iter()
        .map(|&i| session.context()[i].clone())
        .collect();
    // 折叠段太小:排水后仍溢出说明大头在保护/尾部段,折叠无意义
    if compact::estimate_tokens(&source) < compact::MIN_SEGMENT_TOKENS {
        return None;
    }

    // 渲染 + 激进硬截断(本地确定性,不调 LLM)
    let rendered = compact::render_messages(&source);
    let summary = compact::hard_truncate(&rendered, compact::CompactTier::Aggressive);

    // 用一条摘要消息替换折叠段(位置 = 首个被折叠消息的下标,保护段原位保留)
    let first = compact_idx[0];
    let folded = ChatMessage::user(format!(
        "{COMPACT_MARKER_START}\n\
         [溢出恢复折叠 | 覆盖 {} 条历史消息,原文已丢弃]\n\
         {summary}\n\
         {COMPACT_MARKER_END}",
        compact_idx.len(),
    ));
    let compact_set: std::collections::HashSet<usize> =
        compact_idx.iter().copied().collect();
    let mut new_ctx: Vec<ChatMessage> = Vec::with_capacity(len - compact_idx.len() + 1);
    for (i, m) in session.context().iter().enumerate() {
        if i == first {
            new_ctx.push(folded.clone());
        }
        if !compact_set.contains(&i) {
            new_ctx.push(m.clone());
        }
    }
    *session.context_mut() = new_ctx;
    Some(compact_idx.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== is_context_overflow(L1044 正则集) ==========

    #[test]
    fn detects_anthropic_prompt_too_long() {
        let e = AgentError::LlmHttp {
            status: 400,
            retry_after_ms: None,
            message: "HTTP 400 Bad Request: {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"prompt is too long: 54321 tokens > 200000 maximum\"}}".into(),
        };
        assert!(is_context_overflow(&e));
    }

    #[test]
    fn detects_openai_context_length_exceeded() {
        let e = AgentError::LlmHttp {
            status: 400,
            retry_after_ms: None,
            message: "HTTP 400: {\"error\":{\"message\":\"This model's maximum context length is 8192 tokens. However, you requested 10568 tokens\",\"code\":\"context_length_exceeded\"}}".into(),
        };
        assert!(is_context_overflow(&e));
    }

    #[test]
    fn detects_stream_and_generic_carriers() {
        let s = AgentError::LlmStream {
            kind: "invalid_request_error".into(),
            message: "input is too long".into(),
        };
        assert!(is_context_overflow(&s));
        let g = AgentError::Llm("Request failed: input length exceeds context window".into());
        assert!(is_context_overflow(&g));
    }

    #[test]
    fn non_4xx_status_not_overflow() {
        // 500/429 交给重试层与熔断器,本地折叠无意义
        let e = AgentError::LlmHttp {
            status: 500,
            retry_after_ms: None,
            message: "prompt is too long".into(),
        };
        assert!(!is_context_overflow(&e));
        let e429 = AgentError::LlmHttp {
            status: 429,
            retry_after_ms: None,
            message: "too many tokens".into(),
        };
        assert!(!is_context_overflow(&e429));
    }

    #[test]
    fn throttle_and_quota_excluded_even_with_token_words() {
        // pi NON_OVERFLOW_PATTERNS:限流消息可能含 "too many tokens" 字样
        let e = AgentError::LlmHttp {
            status: 400,
            retry_after_ms: None,
            message: "Rate limit reached: too many tokens per minute, please retry".into(),
        };
        assert!(!is_context_overflow(&e));
        let q = AgentError::Llm("quota exceeded: too many tokens this month".into());
        assert!(!is_context_overflow(&q));
    }

    #[test]
    fn unrelated_errors_not_overflow() {
        assert!(!is_context_overflow(&AgentError::Llm("mock down".into())));
        assert!(!is_context_overflow(&AgentError::ToolNotFound("X".into())));
        assert!(!is_context_overflow(&AgentError::LlmHttp {
            status: 400,
            retry_after_ms: None,
            message: "HTTP 400: invalid tool name".into(),
        }));
    }

    // ========== drain_tool_results(Level 1 排水) ==========

    fn session_with_fat_result(fat_chars: usize) -> Session {
        let mut s = Session::new();
        s.context_mut().push(ChatMessage::user("跑命令"));
        s.context_mut().push(ChatMessage::assistant(vec![ContentBlock::ToolUse {
            id: "t1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "seq 1 5000"}),
        }]));
        s.context_mut()
            .push(ChatMessage::tool_result("t1", "x".repeat(fat_chars), false));
        s
    }

    #[test]
    fn drain_truncates_only_oversized_results() {
        let mut s = session_with_fat_result(20_000);
        assert_eq!(drain_tool_results(s.context_mut()), 1);
        // 结构不变:仍是 3 条消息,tool_result 仍配对
        assert_eq!(s.context().len(), 3);
        assert_eq!(s.context()[2].role, Role::Tool);
        // 截短后 ≈ 2000 + 1000 + 标记 < 门槛
        match &s.context()[2].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("overflow-drain"));
                assert!(content.chars().count() < DRAIN_MIN_CHARS);
            }
            other => panic!("应为 ToolResult,实际 {other:?}"),
        }
        // 幂等:二次排水返回 0
        assert_eq!(drain_tool_results(s.context_mut()), 0);
    }

    #[test]
    fn drain_keeps_small_results() {
        let mut s = session_with_fat_result(DRAIN_MIN_CHARS);
        assert_eq!(drain_tool_results(s.context_mut()), 0);
        match &s.context()[2].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content.chars().count(), DRAIN_MIN_CHARS);
            }
            other => panic!("应为 ToolResult,实际 {other:?}"),
        }
    }

    // ========== fold_history(Level 2 折叠) ==========

    /// 构造折叠场景:项目上下文(保护) + 8 条胖历史 + 尾部 4 条普通消息。
    fn fat_session() -> Session {
        let mut s = Session::new();
        s.context_mut().push(ChatMessage::user(format!(
            "{}\n项目说明\n{}",
            crate::agent::project_context::MARKER_START,
            crate::agent::project_context::MARKER_END
        )));
        for i in 0..8 {
            s.context_mut()
                .push(ChatMessage::user(format!("第{i}轮 {}", "话".repeat(400))));
        }
        for i in 0..4 {
            s.context_mut().push(ChatMessage::user(format!("尾部{i}")));
        }
        s
    }

    #[test]
    fn fold_collapses_history_keeps_protected_and_tail() {
        let mut s = fat_session();
        let before = s.context().len();
        let n = fold_history(&mut s).expect("应能折叠");
        assert_eq!(n, 8);
        // 13 条 → 5 条(项目上下文 + 摘要 + 尾部 4 条)
        assert_eq!(s.context().len(), before - n + 1);
        assert!(compact::is_protected(&s.context()[0]), "项目上下文仍在首位");
        // 摘要消息带压缩标记(本身即保护段,天然幂等)
        assert!(compact::is_protected(&s.context()[1]));
        // 尾部 4 条原样保留
        for i in 0..4 {
            assert_eq!(
                s.context()[s.context().len() - 4 + i].content_text(),
                format!("尾部{i}")
            );
        }
        // 二次折叠:候选只剩保护段 → None
        assert!(fold_history(&mut s).is_none());
    }

    #[test]
    fn fold_returns_none_when_no_candidates() {
        // 全部消息都在尾部保护段内
        let mut s = Session::new();
        for i in 0..4 {
            s.context_mut().push(ChatMessage::user(format!("短{i}")));
        }
        assert!(fold_history(&mut s).is_none());
        assert_eq!(s.context().len(), 4);
    }

    #[test]
    fn fold_returns_none_when_segment_too_small() {
        // 非保护历史只有 1 条小消息(估算 < MIN_SEGMENT_TOKENS),折叠无意义
        let mut s = Session::new();
        s.context_mut().push(ChatMessage::user(format!(
            "{}\n项目说明\n{}",
            crate::agent::project_context::MARKER_START,
            crate::agent::project_context::MARKER_END
        )));
        s.context_mut().push(ChatMessage::user("短历史"));
        for i in 0..4 {
            s.context_mut().push(ChatMessage::user(format!("尾部{i}")));
        }
        assert!(fold_history(&mut s).is_none());
    }

    #[test]
    fn fold_pairing_guard_extends_into_trailing_tool_results() {
        // 场景:尾部边界正落在 tool_result 序列中间
        // [proj, u1, a1(tool), r1, u2, a2(tool), r2] len=7
        // 初始 tail_start=3(保护 r1,u2,a2,r2)——但 r1 的 assistant 在折叠段,
        // 守卫必须把 r1 一并纳入折叠,否则产生孤儿 tool_result。
        let mut s = Session::new();
        s.context_mut().push(ChatMessage::user(format!(
            "{}\n项目说明\n{}",
            crate::agent::project_context::MARKER_START,
            crate::agent::project_context::MARKER_END
        )));
        s.context_mut().push(ChatMessage::user(format!("u1 {}", "胖".repeat(400))));
        s.context_mut().push(ChatMessage::assistant(vec![ContentBlock::ToolUse {
            id: "t1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "echo 1"}),
        }]));
        s.context_mut()
            .push(ChatMessage::tool_result("t1", "r1 输出".repeat(100), false));
        s.context_mut().push(ChatMessage::user("u2 当前任务"));
        s.context_mut().push(ChatMessage::assistant(vec![ContentBlock::ToolUse {
            id: "t2".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "echo 2"}),
        }]));
        s.context_mut()
            .push(ChatMessage::tool_result("t2", "r2 输出", false));

        let n = fold_history(&mut s).expect("应能折叠");
        // u1 + a1 + r1(守卫扩入)= 3 条
        assert_eq!(n, 3);
        // 折叠后:proj + 摘要 + u2 + a2 + r2 = 5 条
        assert_eq!(s.context().len(), 5);
        // 无孤儿:每条 Tool 消息的前一条必须是含 ToolUse 的 assistant
        for (i, m) in s.context().iter().enumerate() {
            if m.role == Role::Tool {
                let prev = &s.context()[i - 1];
                assert_eq!(prev.role, Role::Assistant);
                assert!(
                    prev.content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolUse { .. })),
                    "Tool 消息前必须是含 tool_use 的 assistant(无孤儿)"
                );
            }
        }
        // 当前任务 u2 仍在尾部
        assert_eq!(s.context()[2].content_text(), "u2 当前任务");
    }
}
