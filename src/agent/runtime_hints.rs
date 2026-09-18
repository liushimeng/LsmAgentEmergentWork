//! Agent 循环运行时辅助(2026-09-17 自 mod.rs 拆分)。
//!
//! 包含运行时 hint 拼装、截断 stop_reason 判定、首迭代强制工具环境开关与
//! 稳定 JSON 序列化等自由函数,供 [`crate::agent::agent_loop`] 与测试使用。

use super::*;

/// 拼装运行时 hint(2026-09-09 第 09 轮,联动 L771 失败计数早期预警)。
///
/// 仅当对应计数器 > 0 时追加对应行,全 0 时返回空串(零开销)。
/// 拼到 system_prompt 末尾(不破坏 cache_control 缓存前缀;详见
/// 第七轮 PromptCaching 专题),让 LLM 自我感知「正在被短路保护」并主动收敛。
///
/// `<<<LAEW:RUNTIME_HINTS>>>` 标记保证幂等探测 + 与用户提示词严格隔离,
/// 与现有 `LAEW:PROJECT_CONTEXT` / `LAEW:SESSION_HISTORY` / `LAEW:COMPACTED_CONTEXT`
/// 标记风格一致。
pub(crate) fn build_runtime_hints(trace: &ExecutionTrace, consecutive_failures: usize) -> String {
    let mut hints: Vec<String> = Vec::new();
    if trace.truncation_resumes > 0 {
        hints.push(format!(
            "本会话已续接 {} 次截断输出(因 max_tokens 触发),如非必要请缩短回复或减少一次性工具调用。",
            trace.truncation_resumes
        ));
    }
    if trace.overflow_recoveries > 0 {
        hints.push(format!(
            "上下文已自动恢复 {} 次(排水/折叠历史),请避免一次性读取超大文件或拼装超长 prompt。",
            trace.overflow_recoveries
        ));
    }
    if trace.max_tokens_upscalings > 0 {
        hints.push(format!(
            "max_tokens 已升级 {} 次(当前 {} K),这是为解决截断自动翻倍;请控制单次回复长度。",
            trace.max_tokens_upscalings,
            trace.max_tokens_upscalings * 8 // 8K 起,展示近似值即可
        ));
    }
    if consecutive_failures >= 2 {
        hints.push(format!(
            "连续 {} 次工具调用失败,请先停下核对目标参数(路径/工具名/必填字段)再继续,避免在错误路径上重复打转。",
            consecutive_failures
        ));
    }
    if hints.is_empty() {
        return String::new();
    }
    format!(
        "\n\n<<<LAEW:RUNTIME_HINTS>>>\n{}\n<<<END>>>",
        hints.join("\n")
    )
}

/// 判断 `stop_reason` 是否为截断(输出被 token 上限截断)。
///
/// - Anthropic:`"max_tokens"` 表示输出达到 `max_tokens` 上限被截断
/// - OpenAI:`"length"` 表示输出达到 `max_tokens` 上限被截断
pub(super) fn is_truncation_stop_reason(stop_reason: Option<&str>) -> bool {
    matches!(stop_reason, Some("max_tokens") | Some("length"))
}

/// 2026-09-16 第 68 轮 P0-A(修复 v2):首迭代 forced tool 未生效时的强引导 nudge。
/// 触发条件:iter=0 且 first_iter_forced_tool 已设置,但 LLM 返回纯文本(无 tool_use)。
/// 常见原因:Provider/网关拒绝 forced tool_choice 被 resilient 降级为 auto。
/// 文本包含明确的 JSON 参数示例,降低 LLM 首次调用的参数构造门槛。
pub(crate) const FORCED_TOOL_NUDGE_TEXT: &str = "【laew 强制指令】你在首轮回复中没有调用系统要求的工具,这是错误的。\
请立即调用指定工具开始任务,这是硬性要求,不是建议。\
如果你不调用工具,任务将被标记为失败(trace 标 early_terminated)。\
注意:直接用工具规定的 JSON 参数格式调用,不要解释为什么要调用、不要描述计划。\
如果工具返回权限错误(如 macOS -25211),在最终回答中告知用户如何授权,不要放弃任务。";

/// 2026-09-16 第 63 轮(升级):WebUse nudge 改为命令语气。
/// 此前提示语气 LLM 仍可能只回文本,现改为硬性要求 + 直接给出 JSON 参数示例。
pub(crate) const WEB_OPS_NUDGE_TEXT: &str = "【laew 强制指令】你刚才没有调用任何浏览器工具,这是错误的。\n\
请立即调用 BrowserNew 工具打开目标网页。这是硬性要求,不是建议。\n\
参数示例: {\"url\": \"https://目标网址\", \"headless\": true}\n\
如果你不调用 BrowserNew,任务将被标记为失败。\n\
若 BrowserNew 返回 code=3001(未检测到浏览器),立即如实告知用户安装 Chrome/Edge/Chromium,不要编造结果。";

/// 2026-09-16 第 63 轮:WebUse nudge 扩展为多轮触发(iter 1,2,3)。
/// 2026-09-16 第 64 轮:扩展为 1..=6(iter 4-6 用末次警告文本),LLM 在第 4-6 轮
/// 仍只回文本时不再沉默,WebUseRunner 出口兜底确保 trace 标 failed。
pub(crate) fn should_nudge_web_ops(profile_tools: &[&str], iter: usize) -> bool {
    (1..=6).contains(&iter) && profile_tools.iter().any(|t| *t == "BrowserNew")
}

/// 2026-09-16 第 64 轮:WebUse nudge 文本分级(iter ≥ 4 用更严厉措辞 + 终止预告)。
pub(crate) const WEB_OPS_NUDGE_FINAL_TEXT: &str = "【laew 终止预告】你已连续多轮(>=4 次)无浏览器工具调用,任务即将被强制终止。\n\
请立即调用 BrowserNew 工具打开目标网页:\n\
参数: {\"url\": \"https://目标网址\", \"headless\": true}\n\
如果浏览器不存在返回 code=3001,如实告知用户,**不要再输出任何描述性文本**。\n\
继续输出文本而不调用工具 = 任务立即失败,trace 直接标 failed。";

pub(crate) fn web_ops_nudge_text(iter: usize) -> &'static str {
    if iter >= 4 {
        WEB_OPS_NUDGE_FINAL_TEXT
    } else {
        WEB_OPS_NUDGE_TEXT
    }
}

/// 结构化输出强制通道总开关(L6/L19,2026-09-09 第 13 轮)。
///
/// 环境变量 `LAEW_FORCED_TOOLS=off|0|false|no` 关闭 wire 层 forced tool_choice
/// 注入(对齐 `LAEW_INJECTION_GUARD` 惯例);默认开启。关闭后 emit 工具仍在
/// registry,Agent 循环的短路逻辑也保留——模型若仍主动调用 emit 工具同样被接住。
pub(super) fn forced_tools_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        forced_tools_enabled_from(std::env::var("LAEW_FORCED_TOOLS").unwrap_or_default())
    })
}

/// 开关取值解析(独立出来便于单测,OnceLock 缓存进程级一次)。
pub(super) fn forced_tools_enabled_from(raw: String) -> bool {
    !matches!(
        raw.trim().to_lowercase().as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 将 `serde_json::Value` 序列化为「对象 key 排序后的字符串」,作为失败键的稳定摘要。
/// 顺序无关,LLM 调换参数顺序不触发「不同目标」误判。
pub(super) fn stable_json_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let parts: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| format!("{}:{}", k, stable_json_string(v)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(stable_json_string).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}
