//! 对话 Rewind / 分支(D3,2026-09-10 第二十四轮)—— 轮次扫描与截断边界。
//!
//! 知识库来源:第十八轮 D3 维度(pi 树状分支 / deepseek-harness `forkAt(seq)` 轻量派,
//! 方案 `tmpPlan/2026-09-10_07-D3对话Rewind与分支方案.md`)。
//!
//! 主上下文中除了「真实用户提示词」还混有合成 user 消息(项目上下文 / 历史摘要 /
//! 压缩摘要 / 失败回流),本模块负责把真实轮次识别出来并给出安全的截断点,
//! 供 TUI `/rewind` `/undo` `/fork` `/switch` 使用。纯函数层,不依赖 TUI。

use crate::llm::{ChatMessage, ContentBlock, Role};

/// 失败回流消息前缀(orchestrator 回流 Yolo 时 push 的 user 消息)。
const PREVIOUS_FAILURE_PREFIX: &str = "[PREVIOUS_FAILURE]";

/// laew 内部标记统一前缀:项目上下文 / 历史摘要 / 压缩摘要等合成消息均以
/// `<<<LAEW:xxx>>>` 形式包裹,前缀匹配即可覆盖未来新增标记。
const LAEW_MARKER_PREFIX: &str = "<<<LAEW:";

/// 一条真实用户轮次(context 内位置 + 1 起算编号 + 提示词全文)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTurn {
    /// 该轮 user 消息在 `Session.context` 中的下标(截断点)。
    pub ctx_index: usize,
    /// 轮次编号,从旧到新 1 起算(与 `/rewind <N>` 的 N 一致)。
    pub order: usize,
    /// 用户提示词全文(自定义命令时为展开后的 prompt)。
    pub prompt: String,
}

/// 取消息首个 Text 块文本(无文本块返回 None)。
fn first_text(msg: &ChatMessage) -> Option<&str> {
    msg.content.iter().find_map(|b| match b {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    })
}

/// 是否为合成 user 消息(非真实用户轮次)。
///
/// 识别规则:文本包含 `<<<LAEW:`(项目上下文 / 历史摘要 / 压缩摘要等内部标记)
/// 或以 `[PREVIOUS_FAILURE]` 开头(失败回流)。
/// 权衡:用户主动粘贴含 `<<<LAEW:` 的原文会被误判为合成而跳过 —— 极端场景,
/// 见方案文档 §1.3。
pub fn is_synthetic_user_message(msg: &ChatMessage) -> bool {
    if msg.role != Role::User {
        return false;
    }
    match first_text(msg) {
        Some(text) => text.contains(LAEW_MARKER_PREFIX) || text.starts_with(PREVIOUS_FAILURE_PREFIX),
        None => false,
    }
}

/// 扫描上下文中的全部真实用户轮次(从旧到新,order 从 1 起)。
///
/// assistant 回复 / 工具结果 / 合成 user 消息均不计轮次;
/// 轮内追加的 `[PREVIOUS_FAILURE]` 归属其所在轮(截断时随轮移除)。
pub fn scan_user_turns(context: &[ChatMessage]) -> Vec<UserTurn> {
    let mut turns = Vec::new();
    for (idx, msg) in context.iter().enumerate() {
        if is_synthetic_user_message(msg) {
            continue;
        }
        if msg.role != Role::User {
            continue;
        }
        let prompt = first_text(msg).unwrap_or_default().to_string();
        turns.push(UserTurn {
            ctx_index: idx,
            order: turns.len() + 1,
            prompt,
        });
    }
    turns
}

/// 第 `order` 轮的截断点:该轮 user 消息在 context 中的下标。
///
/// 返回 None 表示编号越界(order 为 0 或超过轮次总数)。
/// 截断语义:`context.truncate(boundary)` 保留该轮**之前**的全部消息
/// (头部合成标记天然保留,不会被误删)。
pub fn turn_boundary(context: &[ChatMessage], order: usize) -> Option<usize> {
    if order == 0 {
        return None;
    }
    scan_user_turns(context)
        .into_iter()
        .find(|t| t.order == order)
        .map(|t| t.ctx_index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> ChatMessage {
        ChatMessage::user(text)
    }

    fn assistant(text: &str) -> ChatMessage {
        ChatMessage::assistant(vec![ContentBlock::text(text)])
    }

    /// 模拟真实多轮上下文:项目上下文标记 + 历史标记 + 两轮(user+assistant)+ 轮内失败回流。
    fn sample_context() -> Vec<ChatMessage> {
        vec![
            user("<<<LAEW:PROJECT_CONTEXT>>> 工作目录说明..."),
            user("<<<LAEW:SESSION_HISTORY>>> 上次摘要..."),
            user("第一轮问题"),
            user("[PREVIOUS_FAILURE]\n源: Quality-Check"),
            assistant("第一轮回答"),
            user("第二轮问题"),
            assistant("第二轮回答"),
        ]
    }

    #[test]
    fn empty_context_has_no_turns() {
        assert!(scan_user_turns(&[]).is_empty());
        assert_eq!(turn_boundary(&[], 1), None);
    }

    #[test]
    fn synthetic_messages_are_skipped() {
        let turns = scan_user_turns(&sample_context());
        assert_eq!(turns.len(), 2, "项目上下文/历史/失败回流均不计轮");
        assert_eq!(turns[0].prompt, "第一轮问题");
        assert_eq!(turns[1].prompt, "第二轮问题");
    }

    #[test]
    fn turn_order_and_ctx_index() {
        let ctx = sample_context();
        let turns = scan_user_turns(&ctx);
        assert_eq!(turns[0].order, 1);
        assert_eq!(turns[0].ctx_index, 2, "跳过两个头部合成标记");
        assert_eq!(turns[1].order, 2);
        assert_eq!(turns[1].ctx_index, 5);
    }

    #[test]
    fn assistant_and_tool_messages_not_counted() {
        let ctx = vec![
            user("问"),
            assistant("答"),
            ChatMessage::tool_result("t1", "ok", false),
            user("再问"),
        ];
        let turns = scan_user_turns(&ctx);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[1].ctx_index, 3);
    }

    #[test]
    fn boundary_keeps_head_markers_when_rewind_to_first() {
        let mut ctx = sample_context();
        let boundary = turn_boundary(&ctx, 1).expect("第 1 轮边界");
        ctx.truncate(boundary);
        // 回退到第 1 轮之前:只剩头部两个合成标记
        assert_eq!(ctx.len(), 2);
        assert!(is_synthetic_user_message(&ctx[0]));
        assert!(is_synthetic_user_message(&ctx[1]));
        assert!(scan_user_turns(&ctx).is_empty(), "回退后无真实轮次");
    }

    #[test]
    fn boundary_removes_tail_with_failure_reflow() {
        let mut ctx = sample_context();
        let boundary = turn_boundary(&ctx, 2).expect("第 2 轮边界");
        ctx.truncate(boundary);
        // 保留第 1 轮完整(user + 轮内失败回流 + assistant),移除第 2 轮
        assert_eq!(ctx.len(), 5);
        assert_eq!(scan_user_turns(&ctx).len(), 1);
    }

    #[test]
    fn invalid_order_returns_none() {
        let ctx = sample_context();
        assert_eq!(turn_boundary(&ctx, 0), None, "0 非法");
        assert_eq!(turn_boundary(&ctx, 3), None, "越界");
    }

    #[test]
    fn compacted_context_marker_is_synthetic() {
        let msg = user("<<<LAEW:COMPACTED_CONTEXT>>> 折叠摘要");
        assert!(is_synthetic_user_message(&msg));
    }

    #[test]
    fn non_user_role_never_synthetic() {
        let msg = assistant("<<<LAEW:PROJECT_CONTEXT>>> 看起来像标记");
        assert!(!is_synthetic_user_message(&msg), "仅 user 角色参与判定");
    }

    #[test]
    fn user_text_with_marker_midway_is_synthetic() {
        // 文档化权衡:标记出现在文本任意位置都视为合成
        let msg = user("帮我看看这段 <<<LAEW:PROJECT_CONTEXT>>> 是什么");
        assert!(is_synthetic_user_message(&msg));
    }

    #[test]
    fn multi_block_user_message_uses_first_text() {
        let msg = ChatMessage {
            role: Role::User,
            content: vec![
                ContentBlock::tool_result("t1", "结果", false),
                ContentBlock::text("真实提示词"),
            ],
        };
        assert!(!is_synthetic_user_message(&msg));
        let turns = scan_user_turns(&[msg]);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].prompt, "真实提示词");
    }
}
