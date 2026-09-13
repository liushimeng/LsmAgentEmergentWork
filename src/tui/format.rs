//! TUI 纯函数格式化辅助(自 tui/mod.rs 拆分,2026-09-11,单文件 ≤1800 行规范)。
//!
//! 职责:任务结果两种文本形态(人类版 `format_task_result` / LLM 回填版
//! `format_task_result_for_context`)、用量累加、waiting 心跳文案、
//! CJK 安全截断系(truncate / fit_display / first_line_preview)、
//! 命令相似度建议(levenshtein)、help / record 打印、交互行读取。

use anyhow::Result;

use super::export;
use super::pathfmt;
use crate::config::{Paths, ProviderRecord};
use crate::tui::input::display_width;

/// 格式化任务执行结果为多行文本(D8:屏幕打印与导出同源,避免两处漂移)。
pub(crate) fn format_task_result(
    result: &crate::agent::orchestrator::TaskResult,
    paths: &Paths,
    task_started_at: Option<std::time::Instant>,
) -> String {
    let mut out = String::new();
    let plan_doc_display = result
        .plan_doc
        .as_ref()
        .map(|p| pathfmt::display_path(paths, p))
        .unwrap_or_else(|| "无".into());
    out.push_str(&format!(
        "  [task executed: difficulty={}, plan_doc={}, workflows={}]\n",
        result.classification.task_level.display_name(),
        plan_doc_display,
        result.workflows.len()
    ));
    // 2026-09-10 第二十九轮 P06/M05 自动化测试:
    // TUI 也补一行 Yolo 三步分析摘要,让用户看到 Yolo 怎么理解任务
    // (与 main.rs OrchestrationOutcome::Executed 分支对齐)
    let c = &result.classification;
    let purpose_short = truncate_chars(&c.purpose, 40);
    let goal_short = truncate_chars(&c.goal_summary, 40);
    out.push_str(&format!(
        "  [yolo] purpose={} goal={} intent={} plan_steps={}\n",
        purpose_short,
        goal_short,
        c.intent,
        c.decomposition_plan.len()
    ));
    // 每个 WorkFlow 的 subflow 输出
    for wf in &result.workflows {
        out.push_str(&format!("  --- WorkFlow {} ({}) ---\n", wf.id, wf.name));
        for line in wf.subflow_outcome.lines() {
            out.push_str(&format!("  {line}\n"));
        }
        // Quality-Check 结论
        let (qc_icon, qc_text) = match wf.quality_report.verdict {
            crate::agent::quality::Verdict::Pass => ("✅", "通过"),
            crate::agent::quality::Verdict::Fail => ("❌", "未通过"),
        };
        out.push_str(&format!("  [QC] {qc_icon} {qc_text}\n"));
        if !wf.quality_report.issues.is_empty() {
            for issue in &wf.quality_report.issues {
                out.push_str(&format!("    问题: {issue}\n"));
            }
        }
        // SubAgent 执行轨迹摘要
        if let Some(trace) = &wf.subflow_trace {
            out.push_str(&format!(
                "  [trace] iter={} tools={}(ok={},err={}) early_term={}\n",
                trace.iterations,
                trace.tool_calls,
                trace.tool_calls_ok,
                trace.tool_calls_err,
                trace.early_terminated
            ));
        }
    }
    if !result.summary.is_empty() {
        out.push('\n');
        out.push_str("  [session_context 摘要]\n");
        for line in result.summary.lines() {
            out.push_str(&format!("  {line}\n"));
        }
    }
    // 用量行(与 print_usage 同格式)
    let usage = &result.total_usage;
    if usage.input_tokens > 0 || usage.output_tokens > 0 {
        let mut cache = String::new();
        if usage.cache_read_input_tokens > 0 {
            cache.push_str(&format!("  cache_read={}", usage.cache_read_input_tokens));
        }
        if usage.cache_creation_input_tokens > 0 {
            cache.push_str(&format!(
                "  cache_creation={}",
                usage.cache_creation_input_tokens
            ));
        }
        // 任务总耗时(2026-09-10 第 27 轮 F05 / tmpPlan/2026-09-10_22):
        // 由 print_task_result 调用方从 self.task_started_at.take() 传入,这里直接拼接。
        let elapsed_suffix = task_started_at
            .map(|t| format!("  (耗时 {:.2}s)", t.elapsed().as_secs_f64()))
            .unwrap_or_default();
        out.push_str(&format!(
            "  本次用量: input={}  output={}{}{}\n",
            usage.input_tokens, usage.output_tokens, cache, elapsed_suffix
        ));
    }
    sanitize_terminal_controls(&out)
}

/// 清理人类可读任务结果中的终端控制序列。
///
/// Bash/Vim 等外部工具可能把 CSI 光标移动、OSC 标题、退格等控制字节混入 stdout。
/// 这些字节如果直接 `println!`,会移动光标、覆盖 TUI 底部输入面板,甚至破坏导出文本。
/// 该函数只作用于人类版/导出版;`format_task_result_for_context` 不调用它,
/// 避免改变模型看到的原始工具语义。
pub(crate) fn sanitize_terminal_controls(input: &str) -> String {
    let mut chars = input.chars().peekable();
    let mut out = String::with_capacity(input.len());

    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.peek().copied() {
                // CSI: ESC [ params... final(0x40..=0x7e)
                Some('[') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if ('\u{0040}'..='\u{007e}').contains(&next) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... BEL,或 ESC ] ... ESC \。终止符一并消费。
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{0007}' {
                            break;
                        }
                        if next == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // 两字符 ESC 序列(如 ESC M)。
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            // 输出已按 LF 组织,CR 只会触发覆盖式回绘,丢弃。
            '\r' => {}
            '\n' | '\t' => out.push(c),
            c if ('\u{0000}'..='\u{001f}').contains(&c) => {
                let n = c as u32;
                out.push('^');
                out.push(char::from_u32(0x40 + n).unwrap_or('?'));
            }
            '\u{007f}' => out.push_str("^?"),
            c if ('\u{0080}'..='\u{009f}').contains(&c) => {
                out.push_str(&format!("<U+{:04X}>", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// 构造 Assistant 回填到 LLM 主上下文的精简文本(2026-09-11 第三十轮 BA01/CA31 测试修复)。
///
/// 与 `format_task_result`(人类版,含全部 TUI 元数据)的差异:
/// - **不含** `[task executed: difficulty=...]` 行:任务级别是 orchestrator 内部状态,
///   LLM 看到会困惑(它只知道自己的分类结果,看不到 orchestrator 后续加工)。
/// - **不含** `[yolo] purpose=... goal=... intent=... plan_steps=...` 行:这是上一轮
///   Yolo 的决策文本,会让下轮 Yolo 受上一轮自己决策的暗示(锚定效应),污染任务分类。
/// - **不含** `[trace] iter=... tools=... early_term=...` 行:SubAgent 内部执行统计,
///   LLM 看不到自己的 tool_use_id 与 trace 之间的对应关系,读 trace 反而误导。
/// - **不含** `[session_context 摘要]` 块:摘要已经在 SESSION_HISTORY 标记里注入,
///   再回填相当于让 LLM 看到自己已生成的摘要,造成内容重复膨胀。
/// - **不含** `本次用量: input=... output=... (耗时 ...)` 行:mock 下固定值会让 LLM
///   误以为"上次只用了 140 token",真实 LLM 下暴露自身用量给模型本身也无必要。
/// - **不含** `--- WorkFlow wf-1 (xxx) ---` 标题行:WorkFlow id 与 name 是内部标识,
///   LLM 不关心;只要看到 subflow_outcome 就能理解上轮的产出。
/// - **不含** 行首的 2 空格缩进(避免在 LLM 视角产生伪缩进视觉)。
///
/// **保留**:
/// - 每个 WorkFlow 的 `wf.subflow_outcome`(这是 LLM 必须看到的"上轮真正回答")
/// - QC verdict + issues:让下轮 LLM 知道上次是否成功、有什么遗留问题,指导是否需要重做。
///   QC Pass 给一句话"已通过质检";Fail 给"未通过质检 + 问题清单"。
///
/// 实现要点:
/// - 单 WorkFlow 链路与多 WorkFlow 链路均正常(loop 迭代,各 WorkFlow 拼接)。
/// - 不含本次耗时(`task_started_at` 字段直接丢弃,context 版不关心)。
/// - transcript/导出 仍然用 `format_task_result` 人类版(用户能看到的完整记录)。
pub(crate) fn format_task_result_for_context(
    result: &crate::agent::orchestrator::TaskResult,
) -> String {
    let mut out = String::new();
    for (idx, wf) in result.workflows.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        // SubAgent 的真正回答(LLM 必须看到的核心信息)
        if !wf.subflow_outcome.is_empty() {
            out.push_str(&wf.subflow_outcome);
            if !wf.subflow_outcome.ends_with('\n') {
                out.push('\n');
            }
        }
        // QC verdict:让下轮 LLM 知道上次是否通过质检,指导是否需要重做。
        // 单行精简表达,避免占用过多 token。
        let (verdict_text, issues_text) = match wf.quality_report.verdict {
            crate::agent::quality::Verdict::Pass => ("(质检通过)".to_string(), String::new()),
            crate::agent::quality::Verdict::Fail => {
                let mut issues_str = String::new();
                if !wf.quality_report.issues.is_empty() {
                    issues_str = format!(";问题:{}", wf.quality_report.issues.join("; "));
                }
                ("(质检未通过)".to_string(), issues_str)
            }
        };
        out.push_str(&format!("{verdict_text}{issues_text}\n"));
    }
    out
}
/// Usage 逐字段饱和累加(会话累计用途;orchestrator 内部 add_usage 为私有,此处独立实现)。
pub(crate) fn merge_usage(a: crate::llm::Usage, b: crate::llm::Usage) -> crate::llm::Usage {
    crate::llm::Usage {
        input_tokens: a.input_tokens.saturating_add(b.input_tokens),
        output_tokens: a.output_tokens.saturating_add(b.output_tokens),
        cache_read_input_tokens: a
            .cache_read_input_tokens
            .saturating_add(b.cache_read_input_tokens),
        cache_creation_input_tokens: a
            .cache_creation_input_tokens
            .saturating_add(b.cache_creation_input_tokens),
    }
}

/// 当前本地时间 HH:MM:SS(transcript 轮次时间戳;实现在 export.rs)。
pub(crate) fn now_clock() -> String {
    export::now_clock()
}

/// waiting 心跳行文案(纯函数,便于单测;2026-09-10 第 28 轮 B09/B10 抽取):
/// - `Some(stage)`:阶段内等待,`  [waiting] {stage}  {frame}  ({elapsed}s){slow_warn}`
/// - `None`:初始 spinner(首阶段消息未到),`  [waiting] {frame}  ({elapsed}s){slow_warn}`
/// - elapsed ≥ 60s 追加「等待超过 1 分钟」提示;≥ 30s 追加「响应较慢」。
/// 初始 spinner 同样启用 30s/60s 慢提示 —— 修复前固定 `(0s)`,真实慢 LLM
/// (首字节 >30s)场景下用户既看不到计时也看不到慢提示。
pub(crate) fn waiting_line_text(stage: Option<&str>, frame: char, elapsed_secs: u64) -> String {
    let slow_warn = if elapsed_secs >= 60 {
        " ⚠ 等待超过 1 分钟,可 Ctrl-C 取消"
    } else if elapsed_secs >= 30 {
        " ⚠ 响应较慢"
    } else {
        ""
    };
    match stage {
        Some(stage) => format!("  [waiting] {stage}  {frame}  ({elapsed_secs}s){slow_warn}"),
        None => format!("  [waiting] {frame}  ({elapsed_secs}s){slow_warn}"),
    }
}

/// 把字符串按 char 截断(避免 split_at 在 CJK 多字节上切断),
/// 超长末尾加 `…`。TUI 渲染宽度计算依赖完整 char 边界。
pub(crate) fn truncate_chars(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(limit.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// 建议相似命令（简单的编辑距离近似）。
pub(crate) fn suggest_similar_commands(input: &str) -> Vec<String> {
    let all_commands = [
        "help",
        "h",
        "?",
        "exit",
        "quit",
        "q",
        "clear",
        "c",
        "new",
        "n",
        "model",
        "export",
        "commands",
        "provider",
        "provider list",
        "provider add",
        "provider use",
        "provider del",
        "diff",
        "theme",
        "rewind",
        "undo",
        "fork",
        "branches",
        "switch",
    ];
    let input_lower = input.to_lowercase();
    all_commands
        .iter()
        .filter(|cmd| {
            let cmd_lower = cmd.to_lowercase();
            cmd_lower.starts_with(&input_lower)
                || input_lower.starts_with(&cmd_lower)
                || levenshtein(&cmd_lower, &input_lower) <= 2
        })
        .take(3)
        .map(|s| format!("/{}", s))
        .collect()
}

/// 简单的 Levenshtein 编辑距离。
fn levenshtein(a: &str, b: &str) -> usize {
    let a_len = a.chars().count();
    let b_len = b.chars().count();
    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev_row: Vec<usize> = (0..=b_len).collect();
    let mut curr_row = vec![0usize; b_len + 1];

    for (i, a_char) in a.chars().enumerate() {
        curr_row[0] = i + 1;
        for (j, b_char) in b.chars().enumerate() {
            let cost = if a_char == b_char { 0 } else { 1 };
            curr_row[j + 1] = (prev_row[j + 1] + 1)
                .min(curr_row[j] + 1)
                .min(prev_row[j] + cost);
        }
        std::mem::swap(&mut prev_row, &mut curr_row);
    }
    prev_row[b_len]
}

/// 截断到 max 显示宽度后按显示宽度右侧补空格(CJK 安全),保证 banner 行等宽。
/// 修复 2026-09-10 第 18 轮 P1/P2:「当前模型」行 provider/model 不截断溢出右边框、
/// `{: <N}` 按字符数填充导致含 CJK 内容(如「自动生成(根目录 Markdown)」)时右边界错位。
pub(crate) fn fit_display(s: &str, max: usize) -> String {
    let t = truncate(s, max);
    let w = crate::tui::input::display_width(&t) as usize;
    format!("{}{}", t, " ".repeat(max.saturating_sub(w)))
}

/// 截断字符串到指定显示宽度（简化版，按字符数）。

fn truncate(s: &str, max_len: usize) -> String {
    let w = display_width(s);
    if w as usize <= max_len {
        s.to_string()
    } else {
        // 按显示宽度截断,不在双宽字符中间截断
        let mut out = String::new();
        let mut w = 0u16;
        for c in s.chars() {
            let cw = crate::tui::input::char_width(c);
            if w + cw > max_len as u16 {
                break;
            }
            out.push(c);
            w += cw;
        }
        out + "…"
    }
}

/// 取首行非空内容并压缩空白,截断到 `max` 显示宽度(D3 轮次/分支列表预览用)。
pub(crate) fn first_line_preview(s: &str, max: usize) -> String {
    let first = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let collapsed: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&collapsed, max)
}

/// 简单的标准输入读取（用于交互子命令）。
pub(crate) fn read_line_prompt(prompt: &str) -> Result<String> {
    use std::io::{self, BufRead};
    print!("{prompt}");
    io::Write::flush(&mut io::stdout())?;
    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

pub(crate) fn print_record(r: &ProviderRecord) {
    let marker = if r.is_active { "*" } else { " " };
    println!(
        "  {} id={} [{:>9}] {}/{} @ {}  key={}  ({})",
        marker,
        r.id,
        r.protocol.as_str(),
        r.provider_name,
        r.model_name,
        r.end_point,
        crate::tui::theme::mask_key(&r.api_key),
        r.created_at,
    );
}

pub(crate) fn print_help() {
    println!();
    println!("  ┌──────────────────────────────────────────────────────────┐");
    println!("  │                    laew 可用命令                         │");
    println!("  ├──────────────────────────────────────────────────────────┤");
    println!("  │  命令              说明                                  │");
    println!("  ├──────────────────────────────────────────────────────────┤");
    println!("  │  /help (h, ?)      显示本帮助                            │");
    println!("  │  /exit (quit, q)   退出 TUI                              │");
    println!("  │  /clear (c)        清空对话历史并开启新会话                 │");
    println!("  │  /new (n)          开启新会话(同 /clear)                   │");
    println!("  │  /model            显示当前模型                           │");
    println!("  │  /rewind [N]       列出轮次或回退到第 N 轮之前(自动存分支) │");
    println!("  │  /undo             撤销最后一轮对话                        │");
    println!("  │  /fork             从当前对话分叉出新会话                  │");
    println!("  │  /branches         列出已存分支(/rewind /fork /clear 自动存)│");
    println!("  │  /switch <name>    切换到指定分支                          │");
    println!("  │  /export [path]    导出当前会话(Markdown, .json 后缀 JSON) │");
    println!("  │  /diff <old> <new> 并排 diff 两个文件(行级+字符级着色)    │");
    println!("  │  /theme [kind]     查看或切换主题(D12 a11y 配色)          │");
    println!("  │  /workspace [rf]   查看工作区快照(git/工程/最近改动)       │");
    println!("  │  /commands         列出自定义斜杠命令                      │");
    println!("  │  /provider         管理大模型接入记录(默认进入 list 屏)    │");
    println!("  │  /provider list    列出所有接入记录                       │");
    println!("  │  /provider add     交互式新增接入记录                     │");
    println!("  │  /provider use <id>  切换当前模型                        │");
    println!("  │  /provider del <id>  删除接入记录                        │");
    println!("  ├──────────────────────────────────────────────────────────┤");
    println!("  │  其他输入           作为提示词进入多轮对话                │");
    println!("  └──────────────────────────────────────────────────────────┘");
    println!();
    println!("  @ 文件提及(D1):");
    println!("    @路径            引用文件内容(如 @src/main.rs)");
    println!("    @\"带空格路径\"    引用含空格的路径");
    println!("    @路径#L10-20     只引用指定行区间;@目录 列出目录条目");
    println!("    输入 @ 后 Tab    实时路径补全(目录可继续钻取)");
    println!();
    println!("  补全快捷键:");
    println!("    输入 / 后显示命令列表");
    println!("    ↑ / ↓          上下选择命令");
    println!("    Enter / Tab    接受选中命令");
    println!("    Esc            关闭列表");
    println!();
    println!("  自定义命令:");
    println!("    项目级 .laew/commands/<命令名>.md / 用户级 ~/.laew/commands/<命令名>.md");
    println!("    模板支持 frontmatter(description/argument-hint)与 $ARGUMENTS/$1-$9 占位符");
    println!("    详见 /commands");
}

#[cfg(test)]
mod waiting_line_text_tests {
    use super::*;

    #[test]
    fn initial_spinner_counts_real_elapsed() {
        // 第 28 轮 B09/B10 修复:初始 spinner 的秒数来自 spinner_started_at,
        // 不再固定 (0s) —— 否则首字节 >30s 的真实慢链路下用户以为卡死在 0s。
        assert_eq!(waiting_line_text(None, '⠋', 0), "  [waiting] ⠋  (0s)");
        assert_eq!(waiting_line_text(None, '⠹', 5), "  [waiting] ⠹  (5s)");
    }

    #[test]
    fn initial_spinner_slow_warn_at_30s() {
        // 初始阶段(尚无 stage 消息)同样触发 30s「响应较慢」。
        let s = waiting_line_text(None, '⠸', 31);
        assert!(s.contains("(31s)"), "got: {s}");
        assert!(s.contains("响应较慢"), "got: {s}");
        assert!(!s.contains("1 分钟"), "got: {s}");
    }

    #[test]
    fn stage_waiting_line_label_and_elapsed() {
        let s = waiting_line_text(Some("wf-1 SubAgent 执行中…"), '⠧', 8);
        assert!(
            s.starts_with("  [waiting] wf-1 SubAgent 执行中…  ⠧  (8s)"),
            "got: {s}"
        );
    }

    #[test]
    fn stage_waiting_line_over_one_minute_hint() {
        // 60s 提示优先于 30s 提示,且包含 Ctrl-C 取消指引。
        let s = waiting_line_text(Some("wf-1 QC"), '⠇', 63);
        assert!(s.contains("(63s)"), "got: {s}");
        assert!(s.contains("等待超过 1 分钟"), "got: {s}");
        assert!(s.contains("Ctrl-C 取消"), "got: {s}");
    }

    #[test]
    fn below_30s_has_no_warn_suffix() {
        let s = waiting_line_text(Some("wf-1"), '⠙', 29);
        assert!(!s.contains("⚠"), "got: {s}");
    }
}

#[cfg(test)]
mod terminal_control_sanitizer_tests {
    use super::*;

    #[test]
    fn strips_csi_and_osc_sequences() {
        let input = "before\u{1b}[24;1Hmiddle\u{1b}]0;title\u{07}after";
        assert_eq!(sanitize_terminal_controls(input), "beforemiddleafter");
    }

    #[test]
    fn preserves_newline_and_tab() {
        assert_eq!(sanitize_terminal_controls("a\tb\nc"), "a\tb\nc");
    }

    #[test]
    fn renders_isolated_control_bytes_without_terminal_side_effects() {
        let output = sanitize_terminal_controls("a\u{0008}b\u{0000}c\u{007f}");
        assert_eq!(output, "a^Hb^@c^?");
        assert!(!output.chars().any(|c| c.is_control()));
    }
}

#[cfg(test)]
mod format_task_result_for_context_tests {
    use super::*;
    use crate::agent::orchestrator::{TaskResult, WorkflowResult};
    use crate::agent::quality::{QualityReport, Verdict};
    use crate::llm::Usage;

    fn make_workflow(subflow_outcome: &str, qc_pass: bool, issues: Vec<&str>) -> WorkflowResult {
        WorkflowResult {
            id: "wf-1".into(),
            name: "测试单元".into(),
            subflow_outcome: subflow_outcome.into(),
            quality_report: QualityReport {
                verdict: if qc_pass {
                    Verdict::Pass
                } else {
                    Verdict::Fail
                },
                issues: issues.into_iter().map(|s| s.to_string()).collect(),
                suggestion: String::new(),
                retryable: false,
                source: crate::agent::context::AgentRole::SubAgent,
                evidence: String::new(),
            },
            usage: Usage::default(),
            subflow_trace: None,
        }
    }

    fn make_result(workflows: Vec<WorkflowResult>, summary: &str) -> TaskResult {
        TaskResult {
            goal: "g".into(),
            classification: crate::agent::yolo::TaskClassification {
                task_level: crate::agent::yolo::TaskLevel::Simple,
                purpose: "p".into(),
                goal_summary: "g".into(),
                intent: "info_query".into(),
                agent_role: None,
                decomposition_plan: vec![],
                direct_answer: None,
                user_suggestion_if_fail: String::new(),
                yolo_degraded: false,
            },
            plan_doc: None,
            workflows,
            summary: summary.into(),
            total_usage: Usage::default(),
        }
    }

    #[test]
    fn context_version_excludes_tui_metadata() {
        // 第 30 轮 BA01/CA31 修复:context 版不能含任何 TUI 元数据
        let wf = make_workflow("MOCK_FINAL_ANSWER: hello", true, vec![]);
        let result = make_result(vec![wf], "任务完成摘要");
        let ctx = format_task_result_for_context(&result);
        // 不应含的元数据
        for forbidden in &[
            "[task executed",
            "[yolo]",
            "[trace]",
            "[session_context",
            "本次用量",
            "--- WorkFlow",
            "difficulty=",
            "intent=",
            "plan_steps=",
        ] {
            assert!(
                !ctx.contains(forbidden),
                "context 版不应包含 {forbidden:?},实际: {ctx}"
            );
        }
    }

    #[test]
    fn context_version_includes_subflow_outcome() {
        // 必须保留 subflow_outcome(LLM 要看的"上轮真正回答")
        let wf = make_workflow("这是 SubAgent 的真实回答,包含具体内容。", true, vec![]);
        let result = make_result(vec![wf], "");
        let ctx = format_task_result_for_context(&result);
        assert!(ctx.contains("这是 SubAgent 的真实回答,包含具体内容。"));
    }

    #[test]
    fn context_version_qc_pass_marker() {
        // QC Pass 时含「(质检通过)」标记
        let wf = make_workflow("answer", true, vec![]);
        let result = make_result(vec![wf], "");
        let ctx = format_task_result_for_context(&result);
        assert!(ctx.contains("(质检通过)"));
        // Fail 时不应出现 Pass
        assert!(!ctx.contains("(质检未通过)"));
    }

    #[test]
    fn context_version_qc_fail_marker_with_issues() {
        // QC Fail 时含「(质检未通过)」+ issues 清单
        let wf = make_workflow("bad answer", false, vec!["缺少结论", "数据未引用"]);
        let result = make_result(vec![wf], "");
        let ctx = format_task_result_for_context(&result);
        assert!(ctx.contains("(质检未通过)"));
        assert!(ctx.contains("缺少结论"));
        assert!(ctx.contains("数据未引用"));
        assert!(!ctx.contains("(质检通过)"));
    }

    #[test]
    fn context_version_multi_workflow_joins_with_blank_line() {
        // 多 WorkFlow 链路:每个 WorkFlow 用空行分隔
        let wf1 = make_workflow("first answer", true, vec![]);
        let wf2 = make_workflow("second answer", true, vec![]);
        let result = make_result(vec![wf1, wf2], "");
        let ctx = format_task_result_for_context(&result);
        assert!(ctx.contains("first answer"));
        assert!(ctx.contains("second answer"));
        // 两个 answer 之间有空行
        assert!(ctx.contains("first answer"), "first answer 应在结果中");
        assert!(ctx.contains("second answer"), "second answer 应在结果中");
        // 两个 WorkFlow 之间用空行分隔(中间夹 QC 标记)
        assert!(
            ctx.contains("(质检通过)\n\nsecond answer"),
            "空行分隔两个 WorkFlow,实际: {ctx}"
        );
    }

    #[test]
    fn context_version_empty_subflow_outcome_still_emits_qc_marker() {
        // subflow_outcome 为空时仍输出 QC 标记(下轮 LLM 仍需看到上次质检结果)
        let wf = make_workflow("", true, vec![]);
        let result = make_result(vec![wf], "");
        let ctx = format_task_result_for_context(&result);
        assert!(ctx.contains("(质检通过)"));
    }
}
