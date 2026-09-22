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
    // 第 118 轮:探索预算耗尽提示 —— 在 trace.iterations >= explore_budget 时触发,
    // 提醒 LLM 「进入执行期」,减少重复 inspect/screenshot/eval_js 等只读探查。
    // 调用方(agent_loop.rs)在 `iter == explore_budget` 时通过 trace.explore_budget_exhausted
    // 标记触发本 hint,避免 trace 字段再次修改(向后兼容)。
    if trace.explore_budget_exhausted {
        hints.push(
            "已进入执行期(explore_budget 耗尽)。剩余迭代请专注于 input_text / click / wait 等 \
             写操作,禁止再开新 inspect / screenshot / eval_js 探查(除非 click 后验证)。\
             验证码/阻断请立即 control(request_human, reason=...) 让人工介入。"
                .to_string(),
        );
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
///
/// 2026-09-18 第 87 轮修正:key 必须带 JSON 引号 —— 此前 `{action:"x"}` 无引号
/// 形态不是合法 JSON,导致下游消费者静默失效:
/// - `orchestrator/usage.rs::tool_args_digest`(serde 解析)→ 解析失败返回空,
///   stage「工具调用(最近 N 条)」参数摘要整列丢失;
/// - `tui/format.rs::tool_args_brief` → `extract_json_field` 找不到 `"action":`,
///   [tool] 行退化为 `action=?`。
pub(super) fn stable_json_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let parts: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| {
                    // key 必须是带引号的合法 JSON 字符串(见上方第 87 轮修正说明)
                    let key_json =
                        serde_json::to_string(k).unwrap_or_else(|_| format!("\"{k}\""));
                    format!("{key_json}:{}", stable_json_string(v))
                })
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
