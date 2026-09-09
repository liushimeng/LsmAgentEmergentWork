//! Compact Agent:压缩层(第 8 角色)。
//!
//! 当 Session 主上下文估算 token 数达到当前 Provider `context_max_size` 的 80% 时,
//! 由 Orchestrator 自动触发:把「非保护段」消息交给 Compact Agent(LLM 摘要),
//! 压缩为一条带 `<<<LAEW:COMPACTED_CONTEXT>>>` 标记的摘要消息替换原文。
//!
//! 三档压缩率(按超出幅度自动选档):
//! - Light(轻度):压缩后 ≤ 原文 80%,仅折叠冗长工具输出;
//! - Medium(中度):压缩到 50% 左右,保留主要流程与关键结论;
//! - Aggressive(激进):压缩到 20% 以内,只保目标 / 当前状态 / 关键决策 / 待办。
//!
//! 保护段(永不压缩):带项目上下文 / 历史摘要 / 已压缩摘要 标记的消息 + 最近 4 条消息。
//! LLM 摘要失败时降级为本地硬截断(每条消息保首尾),保证流程不中断。
//!
//! 设计见 `docs/Context设置与自动压缩设计/01-设计与解决方案.md`。

use std::sync::Arc;

use serde::Serialize;

use crate::agent::context::AgentRole;
use crate::agent::{memory, project_context, session_context, Agent, AgentProfile};
use crate::config::{Db, EventType};
use crate::error::Result;
use crate::llm::{ChatMessage, ContentBlock, Usage};
use crate::session::{self, Session};

/// 压缩摘要消息标记(幂等探测 + 保护段识别锚点)。
pub const COMPACT_MARKER_START: &str = "<<<LAEW:COMPACTED_CONTEXT>>>";
/// 压缩摘要消息结束标记。
pub const COMPACT_MARKER_END: &str = "<<<LAEW:COMPACTED_CONTEXT_END>>>";

/// 触发阈值:估算 token ≥ context_max_size × 0.8 时启动压缩。
pub const TRIGGER_RATIO: f64 = 0.8;
/// 尾部保护:最近 N 条消息不参与压缩(借鉴各项目 keep_recent 设计)。
pub const KEEP_RECENT_MESSAGES: usize = 4;
/// 最小压缩段:可压缩内容估算低于该值时压缩无意义(信封开销会反超),直接跳过。
pub const MIN_SEGMENT_TOKENS: usize = 100;

/// 本地 token 估算:字符数 / 4,整体上浮 10% 保守估计。
///
/// 借鉴 Claude Code `roughTokenCountEstimation`(content.length / 4)
/// 与 OpenCode / Pi 的字符数/4 启发式(专题-Context上下文管理深度分析.md §1.1);
/// 上浮 10% 覆盖 CJK 字符与 JSON 结构开销。
pub fn estimate_tokens(messages: &[ChatMessage]) -> usize {
    let chars: usize = messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|b| match b {
            ContentBlock::Text { text } => text.chars().count(),
            ContentBlock::ToolUse { name, input, .. } => {
                name.chars().count() + input.to_string().chars().count()
            }
            ContentBlock::ToolResult { content, .. } => content.chars().count(),
        })
        .sum();
    let base = chars / 4;
    base + base / 10
}

/// 压缩档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactTier {
    /// 轻度:压缩后 ≤ 原文 80%(刚越过阈值)
    Light,
    /// 中度:压缩到 50% 左右(明显超出)
    Medium,
    /// 激进:压缩到 20% 以内(严重超出)
    Aggressive,
}

impl CompactTier {
    /// 按超出幅度自动选档。ratio = est_tokens / context_max_size(调用前保证 ≥ 0.8)。
    pub fn for_ratio(ratio: f64) -> Self {
        if ratio < 1.0 {
            Self::Light
        } else if ratio < 1.5 {
            Self::Medium
        } else {
            Self::Aggressive
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Medium => "medium",
            Self::Aggressive => "aggressive",
        }
    }

    /// 喂给 Compact Agent 的档位指令(与系统提示词三档定义对应)。
    fn instruction(&self) -> &'static str {
        match self {
            Self::Light => "【Light 轻度】目标:压缩后不超过原文的 80%。仅折叠冗长工具输出,对话几乎完整保留。",
            Self::Medium => "【Medium 中度】目标:压缩到原文的 50% 左右。保留主要流程、关键结论、文件路径与命令清单。",
            Self::Aggressive => "【Aggressive 激进】目标:压缩到原文的 20% 以内。只保留目标、当前状态、关键决策与待办。",
        }
    }
}

/// 一次压缩的结果报告。
#[derive(Debug, Clone)]
pub struct CompactReport {
    pub tier: CompactTier,
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub compacted_messages: usize,
    /// true = LLM 摘要失败,走了本地硬截断降级
    pub fallback: bool,
    pub usage: Usage,
}

/// Compact 执行器。
pub struct CompactRunner {
    agent: Agent,
    db: Arc<Db>,
}

impl CompactRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        let agent = Agent::new(llm, AgentProfile::compact_profile());
        Self { agent, db }
    }

    /// 若上下文超过阈值则自动压缩;返回压缩报告(未触发返回 None)。
    ///
    /// LLM 摘要失败时自动降级为本地硬截断,不会把错误抛给上层中断任务。
    pub async fn maybe_compact(
        &self,
        session: &mut Session,
        context_max_size: u64,
    ) -> Result<Option<CompactReport>> {
        if context_max_size == 0 {
            return Ok(None); // 0 = 不限制,关闭自动压缩
        }
        let before_tokens = estimate_tokens(session.context());
        let threshold = (context_max_size as f64 * TRIGGER_RATIO) as usize;
        if before_tokens < threshold {
            return Ok(None);
        }
        let tier = CompactTier::for_ratio(before_tokens as f64 / context_max_size as f64);

        // 划分保护段 / 压缩段
        let len = session.context().len();
        let tail_start = len.saturating_sub(KEEP_RECENT_MESSAGES);
        let mut compact_idx: Vec<usize> = Vec::new();
        for (i, m) in session.context().iter().enumerate() {
            if i >= tail_start || is_protected(m) {
                continue;
            }
            compact_idx.push(i);
        }
        if compact_idx.is_empty() {
            return Ok(None);
        }

        // 渲染压缩段为纯文本
        let source: Vec<ChatMessage> = compact_idx
            .iter()
            .map(|&i| session.context()[i].clone())
            .collect();
        let rendered = render_messages(&source);
        // 可压缩内容太小(主体都在保护段)时跳过:压缩收益抵不过摘要信封开销
        if estimate_tokens(&source) < MIN_SEGMENT_TOKENS {
            return Ok(None);
        }

        // LLM 摘要,失败降级硬截断
        let mut usage = Usage::default();
        let mut fallback = false;
        let mut summary = match self.llm_summarize(tier, &rendered, session.id()).await {
            Ok((text, u)) => {
                usage = u;
                text
            }
            Err(e) => {
                tracing::warn!(error = %e, "Compact LLM 摘要失败,降级为本地硬截断");
                fallback = true;
                hard_truncate(&rendered, tier)
            }
        };
        // 健壮性守卫:摘要比原文还长时(小压缩段 / 模型不守档位),改用硬截断保证只缩不胀
        if summary.chars().count() >= rendered.chars().count() {
            fallback = true;
            summary = hard_truncate(&rendered, tier);
        }

        // 用一条摘要消息替换压缩段(位置 = 首个被压缩消息的下标)
        let first = compact_idx[0];
        let replaced = ChatMessage::user(format!(
            "{COMPACT_MARKER_START}\n\
             [Compact 自动压缩 | 档位={} | 估算 token {} → 目标{:.0}% | 覆盖 {} 条历史消息,原文已丢弃]\n\
             {summary}\n\
             {COMPACT_MARKER_END}",
            tier.as_str(),
            before_tokens,
            tier_target_pct(tier),
            compact_idx.len(),
        ));
        let mut new_ctx: Vec<ChatMessage> = Vec::with_capacity(len - compact_idx.len() + 1);
        let compact_set: std::collections::HashSet<usize> = compact_idx.iter().copied().collect();
        for (i, m) in session.context().iter().enumerate() {
            if i == first {
                new_ctx.push(replaced.clone());
            }
            if !compact_set.contains(&i) {
                new_ctx.push(m.clone());
            }
        }
        let after_tokens = estimate_tokens(&new_ctx);
        *session.context_mut() = new_ctx;

        // 记忆落盘(失败不影响主流程)
        let report = CompactReport {
            tier,
            before_tokens,
            after_tokens,
            compacted_messages: compact_idx.len(),
            fallback,
            usage,
        };
        self.persist(session.id(), &report, &summary);
        Ok(Some(report))
    }

    /// 调 Compact Agent 生成摘要。
    async fn llm_summarize(
        &self,
        tier: CompactTier,
        rendered: &str,
        session_id: &str,
    ) -> Result<(String, Usage)> {
        let prompt = format!(
            "{}\n\n以下是待压缩的对话原文({} 字符):\n\n{rendered}",
            tier.instruction(),
            rendered.chars().count(),
        );
        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(prompt));
        sub_session.id = format!("{session_id}-compact");
        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;
        Ok((text, usage))
    }

    /// 写入 agent_memory + session_memory(审计追踪)。
    fn persist(&self, session_id: &str, report: &CompactReport, summary: &str) {
        let content = format!(
            "[Compact] 档位={} token {}→{} 覆盖 {} 条消息{}",
            report.tier.as_str(),
            report.before_tokens,
            report.after_tokens,
            report.compacted_messages,
            if report.fallback { "(硬截断降级)" } else { "" },
        );
        let _ = self.db.insert_session_memory(&crate::config::SessionMemoryEntry {
            session_id: session_id.to_string(),
            role: AgentRole::Compact,
            event_type: EventType::Summary,
            content,
            usage_input: report.usage.input_tokens,
            usage_output: report.usage.output_tokens,
        });
        let _ = memory::record_entry(
            &self.db,
            AgentRole::Compact,
            session_id,
            &format!("压缩前 est={} tokens", report.before_tokens),
            &summary.chars().take(160).collect::<String>(),
            report.fallback.then_some("LLM 摘要失败,硬截断降级"),
            serde_json::json!({
                "tier": report.tier,
                "before_tokens": report.before_tokens,
                "after_tokens": report.after_tokens,
                "compacted_messages": report.compacted_messages,
            }),
        );
    }
}

/// 消息是否属于保护段(项目上下文 / 历史摘要 / 已压缩摘要)。
pub fn is_protected(m: &ChatMessage) -> bool {
    m.content.iter().any(|b| match b {
        ContentBlock::Text { text } => {
            text.contains(project_context::MARKER_START)
                || text.contains(session_context::HISTORY_MARKER_START)
                || text.contains(COMPACT_MARKER_START)
        }
        _ => false,
    })
}

/// 把消息列表渲染为纯文本(喂给 Compact Agent)。
/// `pub(crate)`:溢出恢复折叠(`overflow::fold_history`)复用同一渲染语义。
pub(crate) fn render_messages(messages: &[ChatMessage]) -> String {
    let mut out = String::new();
    for m in messages {
        let role = match m.role {
            crate::llm::Role::System => "系统",
            crate::llm::Role::User => "用户",
            crate::llm::Role::Assistant => "助手",
            crate::llm::Role::Tool => "工具",
        };
        for b in &m.content {
            match b {
                ContentBlock::Text { text } => {
                    out.push_str(&format!("[{role}] {text}\n"));
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let args: String = input.to_string().chars().take(200).collect();
                    out.push_str(&format!("[{role}] [工具调用] {name}({args})\n"));
                }
                ContentBlock::ToolResult { content, is_error, .. } => {
                    let body: String = content.chars().take(500).collect();
                    let flag = if *is_error { "(失败)" } else { "" };
                    out.push_str(&format!("[{role}] [工具结果{flag}] {body}\n"));
                }
            }
        }
    }
    out
}

/// 降级:本地硬截断(每条渲染行保留前 200 + 后 100 字符),再按档位目标整体截断。
/// `pub(crate)`:溢出恢复折叠(`overflow::fold_history`)复用同一截断语义。
pub(crate) fn hard_truncate(rendered: &str, tier: CompactTier) -> String {
    let mut out = String::new();
    for line in rendered.lines() {
        let n = line.chars().count();
        if n > 320 {
            let head: String = line.chars().take(200).collect();
            let tail: String = line
                .chars()
                .skip(n.saturating_sub(100))
                .collect();
            out.push_str(&format!("{head} ……[省略 {} 字符]…… {tail}\n", n - 300));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    // 按档位目标做总量封顶(按字符近似:token×4);不设下限,保证任何情况下都只缩不胀
    let cap_chars = match tier {
        CompactTier::Light => rendered.chars().count() * 8 / 10,
        CompactTier::Medium => rendered.chars().count() / 2,
        CompactTier::Aggressive => rendered.chars().count() / 5,
    };
    let capped: String = out.chars().take(cap_chars).collect();
    format!("[本地硬截断摘要]\n{capped}")
}

fn tier_target_pct(tier: CompactTier) -> f64 {
    match tier {
        CompactTier::Light => 80.0,
        CompactTier::Medium => 50.0,
        CompactTier::Aggressive => 20.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;

    fn msg(text: &str) -> ChatMessage {
        ChatMessage::user(text)
    }

    #[test]
    fn estimate_tokens_chars_div_4_plus_10pct() {
        // 400 字符 → 100 + 10 = 110
        let m = vec![msg(&"a".repeat(400))];
        assert_eq!(estimate_tokens(&m), 110);
        assert_eq!(estimate_tokens(&[]), 0);
    }

    #[test]
    fn tier_selection_boundaries() {
        assert_eq!(CompactTier::for_ratio(0.8), CompactTier::Light);
        assert_eq!(CompactTier::for_ratio(0.99), CompactTier::Light);
        assert_eq!(CompactTier::for_ratio(1.0), CompactTier::Medium);
        assert_eq!(CompactTier::for_ratio(1.49), CompactTier::Medium);
        assert_eq!(CompactTier::for_ratio(1.5), CompactTier::Aggressive);
        assert_eq!(CompactTier::for_ratio(3.0), CompactTier::Aggressive);
    }

    #[test]
    fn protected_messages_detected() {
        assert!(is_protected(&msg(&format!(
            "{} 项目说明 {}",
            project_context::MARKER_START, project_context::MARKER_END
        ))));
        assert!(is_protected(&msg(&format!(
            "{} 历史 {}",
            session_context::HISTORY_MARKER_START, session_context::HISTORY_MARKER_END
        ))));
        assert!(is_protected(&msg(&format!(
            "{COMPACT_MARKER_START} 摘要 {COMPACT_MARKER_END}"
        ))));
        assert!(!is_protected(&msg("普通对话")));
    }

    #[test]
    fn hard_truncate_respects_tier_cap() {
        let rendered = (0..50)
            .map(|i| format!("[用户] 第{i}轮 {}", "内".repeat(500)))
            .collect::<Vec<_>>()
            .join("\n");
        let light = hard_truncate(&rendered, CompactTier::Light);
        let aggressive = hard_truncate(&rendered, CompactTier::Aggressive);
        assert!(aggressive.chars().count() < light.chars().count());
        assert!(aggressive.contains("[本地硬截断摘要]"));
    }

    struct SummaryLlm;
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for SummaryLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &crate::llm::RequestMeta,
        ) -> Result<crate::llm::Completion> {
            Ok(crate::llm::Completion {
                text: "## 目标\n测试压缩\n\n## 进展与关键结论\n无\n\n## 重要上下文\n无\n\n## 待办\n无".into(),
                tool_calls: vec![],
                usage: Usage { input_tokens: 10, output_tokens: 5, ..Default::default() },
                stop_reason: None,
            })
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    struct FailLlm;
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for FailLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &crate::llm::RequestMeta,
        ) -> Result<crate::llm::Completion> {
            Err(crate::error::AgentError::Llm("mock down".into()))
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    fn fresh_runner(llm: Arc<dyn crate::llm::LlmClient>) -> (CompactRunner, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Arc::new(Db::open(&paths).unwrap());
        (CompactRunner::new(llm, db), dir)
    }

    /// 构造一个超过阈值的 session:1 条项目上下文(保护) + 8 条普通消息。
    fn fat_session(msg_chars: usize) -> Session {
        let mut s = Session::new();
        s.context_mut().push(msg(&format!(
            "{}\n项目说明\n{}",
            project_context::MARKER_START, project_context::MARKER_END
        )));
        for i in 0..8 {
            s.context_mut().push(msg(&format!("第{i}轮 {}", "话".repeat(msg_chars))));
        }
        s
    }

    #[tokio::test]
    async fn compact_triggers_and_replaces_middle() {
        let (runner, _d) = fresh_runner(Arc::new(SummaryLlm));
        let mut s = fat_session(400); // est ≈ (8×410)/4×1.1 ≈ 900+
        let before_len = s.context().len();
        let rep = runner.maybe_compact(&mut s, 100).await.unwrap().expect("应触发压缩");
        assert!(!rep.fallback);
        // 保护:项目上下文(1) + 尾部 4 条 + 摘要 1 条 = 6
        assert_eq!(s.context().len(), before_len - rep.compacted_messages + 1);
        assert_eq!(s.context().len(), 6);
        // 项目上下文仍在首位
        assert!(is_protected(&s.context()[0]));
        // 摘要带标记
        let has_marker = s.context().iter().any(|m| {
            m.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text.contains(COMPACT_MARKER_START)))
        });
        assert!(has_marker);
        assert!(rep.after_tokens < rep.before_tokens);
    }

    #[tokio::test]
    async fn compact_skips_when_below_threshold() {
        let (runner, _d) = fresh_runner(Arc::new(SummaryLlm));
        let mut s = Session::new();
        s.context_mut().push(msg("短对话"));
        assert!(runner.maybe_compact(&mut s, 800_000).await.unwrap().is_none());
        // context_max_size = 0 关闭压缩
        let mut s2 = fat_session(10_000);
        let len = s2.context().len();
        assert!(runner.maybe_compact(&mut s2, 0).await.unwrap().is_none());
        assert_eq!(s2.context().len(), len);
    }

    #[tokio::test]
    async fn compact_fallback_on_llm_failure() {
        let (runner, _d) = fresh_runner(Arc::new(FailLlm));
        let mut s = fat_session(400);
        let rep = runner.maybe_compact(&mut s, 100).await.unwrap().expect("应触发压缩");
        assert!(rep.fallback, "LLM 失败应走硬截断降级");
        let has_marker = s.context().iter().any(|m| {
            m.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text.contains(COMPACT_MARKER_START)))
        });
        assert!(has_marker);
    }

    #[tokio::test]
    async fn compact_persists_memory() {
        let (runner, _d) = fresh_runner(Arc::new(SummaryLlm));
        let mut s = fat_session(400);
        let sid = s.id().to_string();
        runner.maybe_compact(&mut s, 100).await.unwrap();
        let rows = runner.db.list_session_memory(&sid, 10).unwrap();
        assert!(rows.iter().any(|r| r.role == AgentRole::Compact));
        let mem = memory::AgentMemory::load(&runner.db, AgentRole::Compact, &sid, 10);
        assert_eq!(mem.entries.len(), 1);
    }

    struct VerboseLlm;
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for VerboseLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            _meta: &crate::llm::RequestMeta,
        ) -> Result<crate::llm::Completion> {
            // 故意返回超长「摘要」(不守档位),触发守卫
            Ok(crate::llm::Completion {
                text: format!("## 目标\n{}\n", "冗".repeat(5000)),
                tool_calls: vec![],
                usage: Usage::default(),
                stop_reason: None,
            })
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn compact_guard_when_summary_longer_than_source() {
        // LLM 摘要不守档位(比原文还长)时:守卫触发硬截断,压缩后必须变小
        let (runner, _d) = fresh_runner(Arc::new(VerboseLlm));
        let mut s = Session::new();
        // 受保护的项目上下文撑高 est 触发阈值
        s.context_mut().push(msg(&format!(
            "{}\n{}\n{}",
            project_context::MARKER_START,
            "项".repeat(5000),
            project_context::MARKER_END
        )));
        // idx 1:压缩段(600 字符,超过 MIN_SEGMENT_TOKENS)
        s.context_mut().push(msg(&"话".repeat(600)));
        for i in 0..4 {
            s.context_mut().push(msg(&format!("尾部{i}"))); // idx 2..6:尾部保护
        }
        let before = estimate_tokens(s.context());
        let rep = runner.maybe_compact(&mut s, 100).await.unwrap().expect("应触发");
        assert!(rep.fallback, "摘要长于原文应走守卫降级");
        assert!(rep.after_tokens < before, "压缩后必须变小");
    }
}
