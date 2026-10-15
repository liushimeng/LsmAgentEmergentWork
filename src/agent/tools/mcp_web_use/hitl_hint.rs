//! HITL 应答语义辅助(第 145 轮,纯函数,单测在 `tests.rs`)。
//!
//! 背景:内网站点预发布环境实测 —— 人工在弹窗**输入验证码文本**(`z7z2`)后,旧
//! `next_hint` 对 captcha 一律说「人工已完成操作,请 inspect 验证页面状态」,但人工
//! 并没有在页面完成操作、只是报了码;Agent 被误导去 inspect 绕圈而不填码。
//! 本模块按「输码 vs 已完成」分流提示语,由 `control.rs::act_request_human` 的
//! code=0 信封接线(`next_hint` 字段)。
//!
//! 独立子模块的原因:`control.rs` 已是 ≥1700 行临界文件,本轮新增代码按 CLAUDE.md
//! 拆分约定落到职责子模块,防止越 1800 线。
//!
//! 第 155 轮:人工经弹窗输入的可能是**给 Agent 的指示**而非验证码(实测诉求
//! 「人工可通过人工介入窗口输入提示词与 Agent 继续交互」)。`hitl_answer_hint`
//! 增补 `looks_like_code_text` 判据 —— 不像码的文本按指示处理,不填入页面。

/// 应答是否为「选项应答」—— 弹窗选项按钮与 TUI 数字映射共用 `N. <选项文本>`
/// 格式;自由文本(验证码 `z7z2`、动态码数字)不含该前缀。
pub(super) fn is_option_pick_answer(answer: &str) -> bool {
    let t = answer.trim();
    match t.find(". ") {
        Some(i) if i > 0 => t[..i].chars().all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

/// 自由文本是否「像验证码/动态码」(第 155 轮):短(≤16 字符)、无 CJK、
/// 仅字母/数字/空格/连字符/下划线/点(容忍 `993 041` 这类分段码)。
/// 人工写了整句指示(含中文、或更长、或带逗号句号问号)时不该被当码填进页面
/// —— 实测诉求:人工可经弹窗输入提示词与 Agent 继续交互,而非只会报码。
pub(super) fn looks_like_code_text(answer: &str) -> bool {
    let t = answer.trim();
    if t.is_empty() || t.chars().count() > 16 {
        return false;
    }
    if t.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)) {
        return false;
    }
    t.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
}

/// code=0 信封的 `next_hint` 按「输码 vs 指示 vs 已完成」分流:
/// - 自由文本 + captcha/sms/two_factor 且**像码** → 立即 `input_text` 填码提交
///   (不要仅 inspect);
/// - 自由文本 + captcha/sms/two_factor 但**不像码**(整句指示)→ 按指示调整流程,
///   不要把这段文本当作验证码填入页面(第 155 轮);
/// - 选项应答(「N. 选项」形态)→ inspect 验证页面状态后继续;
/// - 其它 reason 自由文本 → 按 human_response 内容继续。
pub(super) fn hitl_answer_hint(reason: &str, answer: &str) -> String {
    if is_option_pick_answer(answer) {
        "人工点选了选项(如「确认我已完成人工操作,交棒」)。请 inspect 验证页面当前状态后继续流程,不要重复 request_human;登录/人工操作完成后核实状态用 inspect,不要 reload —— 风控敏感站点(京东/天猫系)登录后 reload 会触发风控复核弹「当前页面异常」风控墙".to_string()
    } else if matches!(reason, "captcha" | "sms" | "two_factor") {
        if looks_like_code_text(answer) {
            "人工在弹窗输入了验证码/动态码文本(human_response)。请立即用 control(input_text) 把它填入对应输入框并点击提交/登录按钮完成表单,不要仅 inspect 观望".to_string()
        } else {
            "人工在弹窗输入的是一段文字指示而非验证码(human_response)。请按其内容调整流程后再继续,不要把这段文本当作验证码填入页面输入框".to_string()
        }
    } else {
        "人工回复了自由文本(human_response),按其内容继续流程;核实页面状态用 inspect,不要 reload(风控敏感站点登录后 reload 会触发「当前页面异常」风控墙)".to_string()
    }
}

/// 应答是否表达「我已完成人工操作」语义(2026-10-15 闲鱼实测止损输入):
/// 选项 1 文本(新旧文案)或自由文本里的「已完成 / 完成登录 / 操作完成」。
/// 不含单纯「完成」二字 —— 「任务完成」类泛词会误伤。
pub(super) fn is_done_claim_answer(answer: &str) -> bool {
    let t = answer.trim();
    // 选项应答 "1. 确认我已完成人工操作,交棒" 取 ". " 之后的正文再判。
    let body = if is_option_pick_answer(t) {
        match t.find(". ") {
            Some(i) => t[i + 2..].trim(),
            None => t,
        }
    } else {
        t
    };
    body.contains("已完成")
        || body.contains("完成登录")
        || body.contains("登录完成")
        || body.contains("完成操作")
        || body.contains("操作完成")
}

/// 重复「已完成」应答的止损提示(2026-10-15 闲鱼实测根治):同一 page+reason
/// 反复 request_human、人工反复点「已完成」但复验始终未通过时,继续再问只会
/// 空转烧迭代(实测 wf-1 两次空头应答之间烧 8 轮,单元 14 轮/185.9s)。
/// 返回 `Some(追加提示)` 当且仅当:应答是「已完成」语义**且**本 reason 已重复
/// 发起 ≥2 次。由 `hitl_ctl` 拼到 `next_hint` 尾部。
pub(super) fn hitl_repeat_caveat(repeat_count: u32, answer: &str) -> Option<String> {
    if repeat_count < 2 || !is_done_claim_answer(answer) {
        return None;
    }
    Some(format!(
        "⚠️ 此为第 {repeat_count} 次「已完成」应答:此前应答后复验均未通过。若本次 inspect 复验仍未通过,**不要再发起 request_human**(空转会耗尽单元迭代预算),按单元未完成如实收口并向用户说明人工操作未真正生效"
    ))
}
