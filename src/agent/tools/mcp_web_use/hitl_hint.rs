//! HITL 应答语义辅助(第 145 轮,纯函数,单测在 `tests.rs`)。
//!
//! 背景:云智眼预发布环境实测 —— 人工在弹窗**输入验证码文本**(`z7z2`)后,旧
//! `next_hint` 对 captcha 一律说「人工已完成操作,请 inspect 验证页面状态」,但人工
//! 并没有在页面完成操作、只是报了码;Agent 被误导去 inspect 绕圈而不填码。
//! 本模块按「输码 vs 已完成」分流提示语,由 `control.rs::act_request_human` 的
//! code=0 信封接线(`next_hint` 字段)。
//!
//! 独立子模块的原因:`control.rs` 已是 ≥1700 行临界文件,本轮新增代码按 CLAUDE.md
//! 拆分约定落到职责子模块,防止越 1800 线。

/// 应答是否为「选项应答」—— 弹窗选项按钮与 TUI 数字映射共用 `N. <选项文本>`
/// 格式;自由文本(验证码 `z7z2`、动态码数字)不含该前缀。
pub(super) fn is_option_pick_answer(answer: &str) -> bool {
    let t = answer.trim();
    match t.find(". ") {
        Some(i) if i > 0 => t[..i].chars().all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

/// code=0 信封的 `next_hint` 按「输码 vs 已完成」分流:
/// - 自由文本 + captcha/sms/two_factor → 立即 `input_text` 填码提交(不要仅 inspect);
/// - 选项应答(「N. 选项」形态)→ inspect 验证页面状态后继续;
/// - 其它 reason 自由文本 → 按 human_response 内容继续。
pub(super) fn hitl_answer_hint(reason: &str, answer: &str) -> String {
    if is_option_pick_answer(answer) {
        "人工点选了选项(如「我已完成人工操作,继续」)。请 inspect 验证页面当前状态后继续流程,不要重复 request_human".to_string()
    } else if matches!(reason, "captcha" | "sms" | "two_factor") {
        "人工在弹窗输入了验证码/动态码文本(human_response)。请立即用 control(input_text) 把它填入对应输入框并点击提交/登录按钮完成表单,不要仅 inspect 观望".to_string()
    } else {
        "人工回复了自由文本(human_response),按其内容继续流程".to_string()
    }
}
