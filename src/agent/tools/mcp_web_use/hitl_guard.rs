//! 人工介入期间的输入挂起闸(第 152 轮)。
//!
//! 实测事故(2026-10-10 豆包任务,`llaew_20261010_173332.log`):Agent 识别出
//! 「疑似人工验证」→ 发起 `request_human`,页面按 `unlock_page` 切到
//! partial/open 供人工操作;但**提问尚未结束**,Agent 的下一个 `batch` /
//! `sequence` 步骤里的输入类动作照常执行,其收尾的「先解后锁」把页面**重新
//! 锁回 locked**(CDP `Input.setIgnoreInputEvents(true)` + 半透明蒙层)——
//! 人工正在拖的拼图被锁在半路,拖不动、点不了,只能看着倒计时归零。
//!
//! 这不是「Agent 不听话」,而是**两条并行的所有权**没有仲裁:提问期间页面
//! 所有权已移交给人工,Agent 的输入动作就是越权。本模块做最小仲裁:
//!
//! - [`gate`]:HITL 槽位占用期间,输入类动作(复用
//!   `action_dispatches_input` 白名单)与**可变更** `eval_js` 一律拒收;
//! - 观察类(`screenshot` / `inspect` / `eval_js` 只读)与 `request_human`
//!   本身**不拦** —— 人工操作期间 Agent 仍需要看页面;
//! - `params.force=true` 是逃生口(与第 151 轮 6002 硬闸同款约定):
//!   确认无并发人工操作时可强行穿过;
//! - 拒收时给的是 **4003**(「需要人配合,现在请等」),不落在 2xxx/6xxx ——
//!   这不是页面出错,也不是范围越界,而是**等人工**。

use serde_json::{json, Value};


/// 人工介入进行中错误码(第 152 轮)。
///
/// 归入 4xxx「人工介入」段:4001 超时/不可用、4002 人工取消、
/// **4003 人工介入进行中(请等)** —— 同一语义家族。
pub(super) const CODE_HITL_IN_PROGRESS: i32 = 4003;

/// 该 control_action 是否在 HITL 提问期间被挂起(纯函数,可单测)。
///
/// 与 6002 输入硬闸的覆盖面**完全一致**:20 个输入类动作 + 可变更 `eval_js`
/// (LLM 在 `input_text` 碰壁后会把输入迁移到 `execCommand insertText` /
/// 合成事件 / 自己 `btn.click()`)。两处用同一个判定函数,避免「硬闸放行、
/// 挂起闸也放行」的口径漂移。
pub(super) fn should_suspend(action: &str, params: &Value) -> bool {
    crate::agent::browser_overlay::action_dispatches_input(action)
        || (action == "eval_js"
            && params
                .get("expression")
                .and_then(Value::as_str)
                .map(super::blocker_probe::eval_expression_mutates)
                .unwrap_or(false))
}

/// HITL 输入挂起闸。`Some` = 4003 拒收信封(动作不执行)。
///
/// fail-open 条件:未挂载 HITL 枢纽 / 无 pending 槽位 / 动作不在覆盖面 /
/// `params.force=true`,一律 `None` 放行。
pub(super) async fn gate(
    action: &str,
    params: &Value,
) -> Option<crate::error::Result<String>> {
    if params.get("force").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    if !should_suspend(action, params) {
        return None;
    }
    let hub = crate::agent::human_assist::HumanAssistHub::global();
    if !hub.is_pending() {
        return None;
    }
    tracing::warn!(
        action = %action,
        "MCP_Web_Use 输入动作在人工介入进行中被挂起(code=4003)"
    );
    Some(super::envelope(
        CODE_HITL_IN_PROGRESS,
        "人工介入正在进行中,页面当前归人工所有;本动作已挂起以免把人工正在进行的操作锁死",
        json!({
            "suspended_action": action,
            "required_action": "等待人工在弹窗应答后重试本动作;\
                                确认此刻没有人工在操作页面(例如弹窗已超时自灭)时,\
                                可在 params 加 force=true 强制执行。",
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn input_actions_suspend() {
        for a in [
            "click",
            "input_text",
            "key_press",
            "drag",
            "scroll",
            "upload_file",
            "dispatch_event",
        ] {
            assert!(should_suspend(a, &json!({})), "{a} 应被挂起");
        }
    }

    #[test]
    fn observation_actions_pass() {
        for a in ["wait", "screenshot", "set_guard", "heartbeat", "request_human"] {
            assert!(!should_suspend(a, &json!({})), "{a} 不应被挂起");
        }
    }

    #[test]
    fn eval_js_only_suspends_when_mutating() {
        // 只读 eval:放行
        assert!(!should_suspend(
            "eval_js",
            &json!({"expression": "document.querySelectorAll('p').length"})
        ));
        // 可变更 eval:挂起
        for expr in [
            "document.execCommand('insertText')",
            "el.dispatchEvent(new KeyboardEvent('keydown'))",
            "document.querySelector('button').click()",
            "location.href = 'https://x.test'",
        ] {
            assert!(
                should_suspend("eval_js", &json!({"expression": expr})),
                "{expr} 应被挂起"
            );
        }
    }
}
