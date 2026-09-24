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
#[cfg_attr(not(test), allow(dead_code))] // 生产路径走 build_runtime_hints_with,两参封装留给单测
pub(crate) fn build_runtime_hints(trace: &ExecutionTrace, consecutive_failures: usize) -> String {
    let mut ctx = RuntimeHintCtx::default(trace);
    ctx.consecutive_failures = consecutive_failures;
    build_runtime_hints_with(&ctx)
}

/// Thought 显式化提醒阈值(第 120 轮 ReAct 强化)。
///
/// 连续 N 轮「仅 tool_use 无文本」即提醒模型补写 Thought —— 把既有
/// `NO_TEXT_CONVERGE_THRESHOLD=8`(会**终止**循环)的干预点前移到 2 轮,
/// 但**只提醒不终止**:单元预算只有 16 轮,等到第 8 轮才提醒为时已晚。
pub(crate) const THOUGHT_NUDGE_AT: usize = 2;

/// runtime hint 的角色分叉(第 120 轮:根治「代码任务被提示去 click」的语义误导)。
///
/// 此前 `explore_budget` 耗尽文案写死浏览器语义(`inspect/screenshot/eval_js` →
/// `input_text/click/wait`),纯代码/文件类执行单元读到的是**错误指令**。
/// 由 [`crate::agent::profile::AgentProfile::hint_role`] 派生 —— 注意它**不是**纯看
/// 工具面:`builtin_registry()` 无条件含 `MCP_Web_Use`,静态判会把所有代码单元误判成
/// UI 任务,故 `Ui` / `Execute` 的分叉看「本单元**实际调用过**什么工具」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum HintRole {
    /// 浏览器 / 桌面操控执行层(本单元实际调用过 MCP_Web_Use / MCP_Window_Use)。
    #[default]
    Ui,
    /// 通用执行层(代码 / 文件 / 命令类单元)。
    Execute,
    /// 入口层信息收集(Yolo:延迟强制 + emit 通道)。
    Gather,
    /// 判定层(QC / Debug / Compact / SessionContext):无探索/执行阶段之分,不注入。
    Judge,
}

/// [`build_runtime_hints_with`] 的入参集合(第 120 轮)。
///
/// 用结构体而非位置参数:hint 维度已从 2 个涨到 6 个,继续加位置参数会
/// 让调用点变成一串裸 `0` / `None`,可读性与可维护性都塌。
pub(crate) struct RuntimeHintCtx<'a> {
    /// 本单元执行轨迹。
    pub trace: &'a ExecutionTrace,
    /// 连续相同失败次数(既有 L771 预警)。
    pub consecutive_failures: usize,
    /// 角色分叉(决定 explore_budget 耗尽文案)。
    pub role: HintRole,
    /// [`crate::agent::loop_guard::LoopGuard`] 的无进展软提醒(下一轮注入)。
    pub loop_nudge: Option<&'a str>,
    /// 连续「仅 tool_use 无文本」轮数(≥ [`THOUGHT_NUDGE_AT`] 触发 Thought 提醒)。
    pub silent_rounds: usize,
    /// 收口预告(由调用方按 `emit_tool` 有无选文案)。
    pub deadline: Option<&'a str>,
}

impl<'a> RuntimeHintCtx<'a> {
    /// 只带 trace 的默认上下文(等价第 119 轮行为:角色 = Ui,其余信号全空)。
    #[cfg_attr(not(test), allow(dead_code))] // 随 build_runtime_hints 仅测试引用
    pub fn default(trace: &'a ExecutionTrace) -> Self {
        Self {
            trace,
            consecutive_failures: 0,
            role: HintRole::Ui,
            loop_nudge: None,
            silent_rounds: 0,
            deadline: None,
        }
    }
}

/// 拼装运行时 hint(第 120 轮扩展版:角色化 + ReAct 进度 / Thought / 收口)。
///
/// 全部 hint 仍拼在 system **末尾**并用 `<<<LAEW:RUNTIME_HINTS>>>` 包裹,
/// 不破坏 `cache_control` 缓存前缀(第七轮 PromptCaching 专题约束)。
pub(crate) fn build_runtime_hints_with(ctx: &RuntimeHintCtx<'_>) -> String {
    let RuntimeHintCtx {
        trace,
        consecutive_failures,
        role,
        loop_nudge,
        silent_rounds,
        deadline,
    } = ctx;
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
    if *consecutive_failures >= 2 {
        hints.push(format!(
            "连续 {} 次工具调用失败,请先停下核对目标参数(路径/工具名/必填字段)再继续,避免在错误路径上重复打转。",
            consecutive_failures
        ));
    }
    // 第 118 轮:探索预算耗尽提示 —— 在 trace.iterations >= explore_budget 时触发,
    // 提醒 LLM 「进入执行期」,减少重复只读探查。
    // 调用方(agent_loop.rs)在 `iter == explore_budget` 时通过 trace.explore_budget_exhausted
    // 标记触发本 hint,避免 trace 字段再次修改(向后兼容)。
    // 第 120 轮:文案按角色分叉 —— 原文案写死浏览器语义(inspect/click),
    // 纯代码/文件类执行单元读到的是错误指令(D5)。
    if trace.explore_budget_exhausted {
        if let Some(text) = explore_exhausted_text(*role) {
            hints.push(text.to_string());
        }
    }
    // 第 120 轮 ReAct 强化 ①:无进展软提醒(LoopGuard 裁决为 Nudge 后的下一轮注入)。
    if let Some(nudge) = loop_nudge {
        hints.push((*nudge).to_string());
    }
    // 第 120 轮 ReAct 强化 ②:Thought 显式化提醒。
    // 连续 silent_rounds 轮「仅 tool_use 无文本」→ 提醒先写推理再动手。
    // 只提醒不终止(终止仍由 agent_loop 的 NO_TEXT_CONVERGE_THRESHOLD=8 负责)。
    if *silent_rounds >= THOUGHT_NUDGE_AT {
        hints.push(format!(
            "已连续 {silent_rounds} 轮只发工具调用、没有输出任何推理文本。ReAct 要求每轮先用 \
             1~3 句写出 Thought(已经确认了什么 / 还缺什么 / 下一步做哪一件事最省),\
             再发起工具调用;请本轮补上,并显式判断上一步的 Observation 是否让任务向前推进了。"
        ));
    }
    // 第 120 轮 ReAct 强化 ③:迭代进度自感知 —— 预算过半才注入(避免每轮噪音)。
    // 看不见预算的模型不会主动收口(D6):此前只有 Yolo 有收口预告,执行层闷头跑满
    // max_iterations 后被硬杀。
    if trace.max_iterations > 0 && trace.iterations * 2 >= trace.max_iterations {
        hints.push(format!(
            "【进度】第 {}/{} 轮迭代,已调用工具 {} 次(成功 {} / 失败 {})。\
             若已达成 expected_output 请立即收口输出终答,不要为「看起来更完整」继续探索。",
            trace.iterations, trace.max_iterations, trace.tool_calls, trace.tool_calls_ok,
            trace.tool_calls_err
        ));
    }
    // 第 120 轮 ReAct 强化 ④:收口预告(倒数第二轮),文案由调用方按 emit_tool 有无选定。
    if let Some(text) = deadline {
        hints.push((*text).to_string());
    }
    if hints.is_empty() {
        return String::new();
    }
    format!(
        "\n\n<<<LAEW:RUNTIME_HINTS>>>\n{}\n<<<END>>>",
        hints.join("\n")
    )
}

/// 探索预算耗尽文案(第 120 轮角色化)。`Judge` 返回 `None` = 不注入。
fn explore_exhausted_text(role: HintRole) -> Option<&'static str> {
    match role {
        HintRole::Ui => Some(
            "已进入执行期(explore_budget 耗尽)。剩余迭代请专注于 input_text / click / wait 等 \
             写操作,禁止再开新 inspect / screenshot / eval_js 探查(除非 click 后验证)。\
             验证码/阻断请立即 control(request_human, reason=...) 让人工介入。",
        ),
        HintRole::Execute => Some(
            "已进入执行期(explore_budget 耗尽)。剩余迭代请专注于**落地改动与验证**\
             (Write / Edit / Bash 构建与测试),禁止再开新的只读探查(Read / Glob / Grep),\
             除非是为了验证刚做的改动。已收集的信息足够动手了。",
        ),
        HintRole::Gather => Some(
            "信息收集预算已耗尽(explore_budget 耗尽)。请立即停止新的探查,\
             基于已收集的信息收口提交结论。",
        ),
        HintRole::Judge => None,
    }
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
