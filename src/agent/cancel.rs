//! 取消传播原语与中断后的消息一致性修复。
//!
//! 设计依据:
//! - `docs/Agent源码调研/专题-第五轮-中断取消与后台任务深度分析.md`
//!   §2.1 atomcode 三件套传播 + §6.2「中断消息也是一条 user 消息」
//! - `docs/Agent源码调研/专题-第六轮-SubAgent调度与并发模型深度对比.md` §11.2
//! - 方案:`tmpPlan/2026-09-08_07-取消传播与优雅中断方案.md`
//!
//! 原语选用 `tokio_util::sync::CancellationToken`(atomcode 同款,支持
//! `child_token()` 树形传播)。统一别名 `CancelToken`,后续若更换原语只动此处。

pub use tokio_util::sync::CancellationToken as CancelToken;

use crate::llm::ChatMessage;

/// 取消时补全 orphan tool_use 的标准文案(atomcode `message.rs:438` 语义)。
pub const CANCELLED_TOOL_RESULT: &str = "(cancelled — 工具被用户中断,副作用未知)";

/// 中断后的消息一致性修复:为上下文中所有「已发出 tool_use 但缺少 tool_result」
/// 的调用补一条 `is_error=true` 的 tool_result。
///
/// 协议一致性是硬约束:Anthropic / OpenAI 都会拒绝含 orphan tool_use 的
/// 下一次请求(400)。任何取消出口(工具执行中断 / LLM 调用中断)都必须先
/// 调用本函数再上抛。
///
/// 返回补全的条数(0 表示上下文本已配对,无操作)。
pub fn backfill_cancelled_tool_results(ctx: &mut Vec<ChatMessage>) -> usize {
    // 1) 收集全部 tool_use id(按出现顺序)
    let mut pending: Vec<String> = Vec::new();
    for msg in ctx.iter() {
        if msg.role == crate::llm::Role::Assistant {
            for block in &msg.content {
                if let crate::llm::ContentBlock::ToolUse { id, .. } = block {
                    pending.push(id.clone());
                }
            }
        }
    }
    if pending.is_empty() {
        return 0;
    }
    // 2) 剔除已有 tool_result 的 id(回填消息角色为 Role::Tool,见 ChatMessage::tool_result)
    for msg in ctx.iter() {
        if msg.role == crate::llm::Role::Tool {
            for block in &msg.content {
                if let crate::llm::ContentBlock::ToolResult { tool_use_id, .. } = block {
                    pending.retain(|id| id != tool_use_id);
                }
            }
        }
    }
    if pending.is_empty() {
        return 0;
    }
    // 3) 逐条补全(tool_result 每条独立消息,与 run_session 正常回填形态一致)
    let n = pending.len();
    for id in pending {
        ctx.push(ChatMessage::tool_result(id, CANCELLED_TOOL_RESULT, true));
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatMessage, ContentBlock};

    fn assistant_with_tools(ids: &[&str]) -> ChatMessage {
        ChatMessage::assistant(
            ids.iter()
                .map(|id| ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "echo hi"}),
                })
                .collect(),
        )
    }

    #[test]
    fn backfill_fills_orphan_tool_use() {
        let mut ctx = vec![
            ChatMessage::user("跑一下"),
            assistant_with_tools(&["t-1"]),
        ];
        let n = backfill_cancelled_tool_results(&mut ctx);
        assert_eq!(n, 1);
        assert_eq!(ctx.len(), 3);
        let last = ctx.last().unwrap();
        assert!(matches!(&last.content[0],
            ContentBlock::ToolResult { tool_use_id, is_error, .. }
            if tool_use_id == "t-1" && *is_error));
    }

    #[test]
    fn backfill_noop_when_all_paired() {
        let mut ctx = vec![
            ChatMessage::user("跑一下"),
            assistant_with_tools(&["t-1"]),
            ChatMessage::tool_result("t-1", "ok", false),
        ];
        assert_eq!(backfill_cancelled_tool_results(&mut ctx), 0);
        assert_eq!(ctx.len(), 3);
    }

    #[test]
    fn backfill_fills_multiple_orphans_and_keeps_paired() {
        let mut ctx = vec![
            ChatMessage::user("多工具"),
            assistant_with_tools(&["t-1", "t-2"]),
            ChatMessage::tool_result("t-1", "ok", false), // t-1 已回填,t-2 是孤儿
            ChatMessage::assistant(vec![ContentBlock::text("继续")]),
            assistant_with_tools(&["t-3"]),
        ];
        let n = backfill_cancelled_tool_results(&mut ctx);
        assert_eq!(n, 2);
        // 末尾两条是 t-2 / t-3 的取消回填
        let tail: Vec<&str> = ctx[ctx.len() - 2..]
            .iter()
            .map(|m| match &m.content[0] {
                ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.as_str(),
                _ => panic!("应为 tool_result"),
            })
            .collect();
        assert_eq!(tail, vec!["t-2", "t-3"]);
    }

    #[test]
    fn backfill_noop_on_plain_conversation() {
        let mut ctx = vec![
            ChatMessage::user("你好"),
            ChatMessage::assistant(vec![ContentBlock::text("你好!")]),
        ];
        assert_eq!(backfill_cancelled_tool_results(&mut ctx), 0);
    }
}
