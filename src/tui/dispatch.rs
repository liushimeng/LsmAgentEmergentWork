//! TUI 输入分发与任务结果输出(自 tui/mod.rs 拆分,2026-09-11,单文件 ≤1800 行规范)。
//!
//! 职责:`handle_user_input` 入口分流(`?` 别名 / 斜杠命令 / 普通提示词)→
//! `dispatch_prompt` 统一任务分发(@ 提及展开 / 阶段进度协程 / SIGINT 取消 /
//! debug 报告 / 终态打印 / transcript 记录),以及结果渲染 `print_*` 家族。

use std::sync::Arc;

use anyhow::Result;

use super::TuiSession;
use super::export::{OutcomeKind, TranscriptEntry};
use super::format::{
    format_task_result, format_task_result_for_context, merge_usage, now_clock,
    waiting_line_text,
};
use super::pathfmt;
use super::theme::{attrs_to_ansi, bg_color_ansi, color_to_ansi256};
use crate::agent::debug::{DebugCollector, ReportMeta, finalize_report};
use crate::agent::orchestrator::OrchestrationOutcome;
use crate::llm::ChatMessage;

impl TuiSession {
    pub async fn handle_user_input(&mut self, line: &str) -> Result<bool> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(false);
        }
        // 单字符 `?` 等价 `/help`(文档/补全都列出此别名,但因无 `/` 前缀原本走不到 handle_slash,
        // 导致输入 `?` 被当作普通 prompt 处理,本轮修复接通)。
        if line == "?" {
            return self.handle_slash("?", line).await;
        }
        if let Some(rest) = line.strip_prefix('/') {
            // 斜杠命令(line 原文传给 handle_slash,自定义命令 dispatch 时记录原始输入)
            return self.handle_slash(rest, line).await;
        }
        self.dispatch_prompt(line, line).await
    }

    /// 统一提示词分发(D2/D8,2026-09-10):
    /// - `raw`:用户原始输入行(自定义命令记 `/cmd args` 原文,transcript/导出使用);
    /// - `prompt`:实际送入编排的提示词(自定义命令展开后,可能与 raw 不同)。
    ///
    /// 普通输入与自定义命令都汇入此处,保证取消/调试/输出/transcript 记录单点收敛。
    pub(crate) async fn dispatch_prompt(&mut self, raw: &str, prompt: &str) -> Result<bool> {
        let turn_ts = now_clock();
        // D1 @ 提及展开(2026-09-10 第二十八轮,L1426):@路径/@"带空格"/@路径#L10-20
        // 命中真实文件时以 <<<LAEW:ATTACHMENTS>>> 附件块追加到送入上下文的消息;
        // transcript/导出仍记 raw 原文,不受影响。
        let expanded = crate::agent::attachments::expand_mentions(prompt, &self.paths.work_dir);
        let prompt_owned;
        let prompt = if expanded.attached > 0 || !expanded.missed.is_empty() {
            if expanded.attached > 0 {
                println!("  [附件] 已附加 {} 个 @ 提及内容", expanded.attached);
            }
            for m in &expanded.missed {
                println!("  [附件] 跳过 {m}");
            }
            prompt_owned = expanded.message;
            prompt_owned.as_str()
        } else {
            prompt
        };
        // 普通提示词:Orchestrator 编排(可取消:Ctrl-C 经 SIGINT 自动感知,零新增命令)
        self.session.context_mut().push(ChatMessage::user(prompt));
        // debug 模式:每个任务开始前重置采集器
        if let Some(collector) = &self.debug {
            collector.reset(self.session.id());
        }
        println!("  [orchestrator 调度中... Ctrl-C 取消]");
        // 任务开始时间戳(2026-09-10 第 27 轮 F05 / tmpPlan/2026-09-10_22):
        // 用于在「本次用量」行末尾追加总耗时,便于用户感知 LLM 响应速度。
        self.task_started_at = Some(std::time::Instant::now());
        // 阶段进度打印协程(2026-09-10 第 23 轮,D05/D07 测试轮 + 第 26 轮 F05/C06):
        // - 阶段切换:收到消息挂起 1.5s 再显示;任务快速完成(所有发送端 drop → recv
        //   返回 None)时挂起消息直接丢弃 —— mock 级链路零噪音,真实 LLM 长任务稳定可见。
        // - 阶段内 waiting 心跳(2026-09-10 第 26 轮 F05/C06 测试轮):单阶段执行超过
        //   1s 后每秒重写一行 [waiting] 行,显示 spinner + 已等待秒数;30s 加「响应较慢」
        //   提示,60s 加「Ctrl-C 取消」提示;阶段切换时自动擦除旧行不留痕迹。
        // - 2026-09-10 第 27 轮 F05 测试轮(F05_p1.md / tmpPlan/2026-09-10_22):
        //   提交后立刻打印初始 spinner(无 stage 文字,只 ⠋ (0s)),消除前 1.5s 的空窗;
        //   真实 LLM 慢场景下用户不再感觉「卡了」。
        let (stage_tx, mut stage_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let stdout_is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
        let stage_printer = tokio::spawn(async move {
            use std::io::Write as _;
            let hold = std::time::Duration::from_millis(1500);
            let tick = std::time::Duration::from_millis(1000);
            // 队列语义:同批到达的阶段消息(如 Yolo 分类 + SubAgent 启动)整批冲刷,
            // 不互相覆盖;通道关闭(任务结束)早于计时器触发时全部丢弃。
            // 2026-09-10 第 27 轮 F05:启动后 1.5s hold 仍生效,目的是让快速任务(mmock < 1s 完成)
            // 阶段消息直接被静默丢弃,屏幕零噪音。真实慢任务场景下,初始 spinner 立刻占位
            // 1.5s 空窗;阶段消息在 hold 窗口外到达才打印(原 hold 语义不变)。
            let mut queue: std::collections::VecDeque<String> = Default::default();
            let mut idle = Box::pin(tokio::time::sleep(tick));
            // 当前阶段的 waiting 心跳状态:None 表示无活动阶段
            let mut current_stage: Option<(String, std::time::Instant)> = None;
            // 初始 spinner 状态:协程启动后立刻打印 ⠋ (0s) 占位,收到第一条
            // 阶段消息时清掉 + 进入正常 stage/waiting 心跳。
            let mut initial_spinner_active: bool = true;
            // 初始 spinner 的计时起点:首个 LLM 请求在途(尚无 stage 消息)期间用它
            // 计算真实等待秒数,30s/60s 慢提示在「首字节未到」阶段同样生效
            // (2026-09-10 第 28 轮 B09/B10 修复:此前固定 (0s),慢提示永不触发)。
            let spinner_started_at = std::time::Instant::now();
            let mut spinner_idx: usize = 0;
            // Braille Pattern 字符集(Braille spinner,宽 1,绝大多数 Unicode 终端可见)
            const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

            // ANSI 纪律(2026-09-10 第 28 轮 B09/B10 慢链路实测修复):
            // waiting 行(初始 spinner / 心跳行)永远以 \r 开头「原地重写」—— 光标停在
            // 该行行中,不换行。因此任何 println! 输出([stage] 行 / 最终结果)之前必须
            // 先 \r\x1b[K 把该行从列 0 清掉,再从列 0 打印;**绝不**用 \x1b[1A 上移清除
            // (旧实现会误删 waiting 行上方刚打印的 stage 历史行,并让后续 println 落在
            // 行中部 —— 实测回滚区丢两行 stage + 新行缩进到第 50/79 列)。
            // waiting_line_on_screen 精确追踪屏幕上是否有一行待清的原地 waiting 行。
            let mut waiting_line_on_screen: bool = true; // 初始 spinner 已打印
            let clear_waiting_line = |tty: bool| {
                if tty {
                    let _ = std::io::stdout().write_all(b"\r\x1b[K");
                    let _ = std::io::stdout().flush();
                }
            };
            // 工具函数:原地重写当前 waiting 行(\r + 整行覆盖 + flush)
            let rewrite_waiting_line = |tty: bool, line: &str| {
                if tty {
                    let _ = std::io::stdout().write_all(b"\r");
                    let _ = std::io::stdout().write_all(line.as_bytes());
                    let _ = std::io::stdout().write_all(b"\x1b[K");
                    let _ = std::io::stdout().flush();
                }
            };

            // 启动时立即打印初始 spinner 占位,占满 1.5s 空窗(2026-09-10 第 27 轮 F05)。
            // 用 \r 开头 + 不换行,后续 rewrite_waiting_line(\r) 可在同一行原地刷新;
            // 收尾时用整行清除(\r\x1b[K)即可擦掉,不会留下残影。
            if stdout_is_tty {
                let _ = std::io::stdout().write_all(b"\r  [waiting] \xe2\xa0\x8b  (0s)\x1b[K");
                let _ = std::io::stdout().flush();
            }

            loop {
                tokio::select! {
                    maybe = stage_rx.recv() => match maybe {
                        Some(line) => {
                            // 不在此处清除 waiting 行:清除统一延迟到 idle flush,
                            // 否则「recv 清行 → 心跳用过期 current_stage 复活旧行 →
                            // flush println 落在行中部」的竞态会复发行中部错位
                            // (心跳分支另有 queue.is_empty() 闸门双保险)。
                            if queue.is_empty() {
                                idle.as_mut().reset(tokio::time::Instant::now() + hold);
                            }
                            queue.push_back(line);
                        }
                        None => {
                            // 任务结束:清除屏幕上的 waiting 行,丢弃未显示的快速阶段
                            if waiting_line_on_screen {
                                clear_waiting_line(stdout_is_tty);
                            }
                            break;
                        }
                    },
                    _ = &mut idle => {
                        // 阶段切换 flush:第一批 stage 行打印前先清 waiting 行
                        // (统一 \r\x1b[K,从列 0 打印,不吞历史行)。
                        while let Some(line) = queue.pop_front() {
                            if waiting_line_on_screen {
                                clear_waiting_line(stdout_is_tty);
                                waiting_line_on_screen = false;
                                initial_spinner_active = false;
                            }
                            println!("  [stage] {line}");
                            let _ = std::io::stdout().flush();
                            current_stage = Some((line, std::time::Instant::now()));
                        }
                        // 阶段内 waiting 心跳:每 1s 重写一行(覆盖上一帧)。
                        // queue 非空(有待冲刷阶段)时绝不重写 —— 防止用旧
                        // current_stage 把已清掉的行复活,导致下一条 [stage] 错位。
                        if queue.is_empty() {
                            let (text, next) =
                                if let Some((stage, started)) = &current_stage {
                                    (
                                        waiting_line_text(
                                            Some(stage),
                                            SPINNER[spinner_idx % SPINNER.len()],
                                            started.elapsed().as_secs(),
                                        ),
                                        tick,
                                    )
                                } else if initial_spinner_active {
                                    (
                                        waiting_line_text(
                                            None,
                                            SPINNER[spinner_idx % SPINNER.len()],
                                            spinner_started_at.elapsed().as_secs(),
                                        ),
                                        tick,
                                    )
                                } else {
                                    // 无活动阶段:回到 1.5s 节奏(基本不会进入此分支,防御性)
                                    (String::new(), hold)
                                };
                            if !text.is_empty() {
                                spinner_idx += 1;
                                rewrite_waiting_line(stdout_is_tty, &text);
                                waiting_line_on_screen = true;
                            }
                            idle.as_mut().reset(tokio::time::Instant::now() + next);
                        }
                    }
                }
            }
        });
        let cancel = crate::agent::cancel::CancelToken::new();
        // 任务窗口 SIGINT 监听:第一次中断取消当前任务(claudecode 语义),
        // 第二次强制退出(exit 130)。输入等待窗口由 InputHandler raw mode
        // 按键事件处理(既有 Interrupted 行为,不变)。
        let sig_cancel = cancel.clone();
        let sig_task = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            println!();
            println!("  [laew] 收到中断信号,正在取消当前任务(再次 Ctrl-C 强制退出)...");
            sig_cancel.cancel();
            let _ = tokio::signal::ctrl_c().await;
            std::process::exit(130);
        });
        let handle_result = self
            .orchestrator
            .handle_cancellable_with_progress(&mut self.session, &cancel, Some(stage_tx))
            .await;
        // 任务结束:撤掉 SIGINT 监听,避免游离监听吞掉后续按键窗口外的信号
        sig_task.abort();
        // 阶段打印协程收尾:通道已随任务结束关闭,快速阶段静默丢弃后再输出结果块,
        // 保证 [stage] 行不会插进最终结果中间
        let _ = stage_printer.await;
        // debug 模式:任务结束(无论成败)后生成 Debug 报告
        if let (Some(collector), Some(raw_llm)) = (&self.debug, &self.debug_llm_raw) {
            self.emit_debug_report(collector, raw_llm.clone(), prompt, &handle_result)
                .await;
        }
        // 终态:屏幕输出 + transcript 记录(outcome, response, 本轮用量)
        let entry = match &handle_result {
            Ok(OrchestrationOutcome::DirectAnswer { text, usage, .. }) => {
                self.print_assistant_text_with_agent("Yolo", text, usage);
                Some((
                    OutcomeKind::DirectAnswer,
                    text.clone(),
                    text.clone(),
                    *usage,
                ))
            }
            Ok(OrchestrationOutcome::Executed { result }) => {
                // print_task_result 内部从 self.task_started_at.take() 取值后拼接耗时,
                // 这里先 format 一份给 transcript 使用,不再重复 take(避免耗时被吃掉)。
                // 2026-09-11 第三十轮 BA01/CA31 测试修复:
                // 多轮 Assistant 回填必须用 context 版(仅 subflow_outcome + QC verdict),
                // 不能复用人类版(含 [task executed]/[yolo]/[trace]/[session_context]/本次用量
                // 等 TUI 元数据会污染 LLM 决策、浪费 token、跨轮指代干扰)。
                // print_task_result 继续用人类版(含 trace 等可观测性信息,屏幕友好)。
                // transcript/导出也用人类版(用户看到的完整记录)。
                let transcript_text =
                    format_task_result(result, &self.paths, self.task_started_at);
                let context_text = format_task_result_for_context(result);
                self.print_task_result(result);
                Some((
                    OutcomeKind::Executed,
                    context_text,
                    transcript_text,
                    result.total_usage,
                ))
            }
            Ok(OrchestrationOutcome::Failed {
                suggestion,
                reason,
                usage,
                classification,
            }) => {
                println!();
                println!("  [agent failed]");
                // 2026-09-11 第三十八轮 BUG-2:失败路径补 [yolo] 分类摘要(与
                // Executed 路径对齐)—— 失败前任务按什么难度/目标执行过,用户应可见,
                // 不应随 Failed 分支的 `..` 解构一起丢失。
                let c = &classification;
                println!(
                    "  [yolo] difficulty={} purpose={} goal={} intent={} plan_steps={}",
                    c.task_level.display_name(),
                    crate::tui::format::truncate_chars(&c.purpose, 40),
                    crate::tui::format::truncate_chars(&c.goal_summary, 40),
                    c.intent,
                    c.decomposition_plan.len()
                );
                // F4(2026-09-10 第 25 轮):先呈现真实失败原因,再给建议 ——
                // 此前只显示 suggestion,可能与真实失败无关而误导用户。
                if !reason.is_empty() {
                    println!("  原因: {reason}");
                }
                println!("  建议: {suggestion}");
                // 2026-09-10 第 22 轮:与 Executed/DirectAnswer 路径对齐,统一调用
                // print_usage 输出(input/output + cache_read + cache_creation),
                // 避免 Failed 路径用量维度缺失,导致用户看不到 prompt caching 命中量。
                self.print_usage(usage);
                let mut text = String::from("执行失败。");
                if !reason.is_empty() {
                    text.push_str(&format!("\n原因: {reason}"));
                }
                text.push_str(&format!("\n建议: {suggestion}"));
                Some((
                    OutcomeKind::Failed,
                    text.clone(),
                    text,
                    *usage,
                ))
            }
            Err(e) if matches!(e, crate::error::AgentError::Cancelled) => {
                println!();
                println!("  ✓ 本次任务已取消,可继续输入新指令。");
                Some((
                    OutcomeKind::Cancelled,
                    "(任务已取消)".to_string(),
                    "(任务已取消)".to_string(),
                    crate::llm::Usage::default(),
                ))
            }
            Err(e) => {
                eprintln!("  [agent error] {e}");
                let text = format!("[agent error] {e}");
                Some((
                    OutcomeKind::Error,
                    text.clone(),
                    text,
                    crate::llm::Usage::default(),
                ))
            }
        };
        if let Some((outcome, context_response, transcript_response, usage)) = entry {
            self.session_usage = merge_usage(self.session_usage, usage);
            // 多轮对话记忆(2026-09-10 第 23 轮):最终回答回填主上下文。
            // 此前只有 user 提示词进 session.context(),assistant 回复从不回填,
            // 下轮 Yolo 看不到模型自己上轮的回答,「你上面的比方里…」类指代追问
            // 必然失忆(实测 mock 日志第 2 轮请求全是 user 角色)。回填文本与
            // transcript/导出分流:上下文只保留回答与 QC 结论,导出保留完整人类版。
            let blocks = vec![crate::llm::ContentBlock::text(context_response.clone())];
            self.session
                .context_mut()
                .push(crate::llm::ChatMessage::assistant(blocks));
            self.transcript.push(TranscriptEntry {
                ts: turn_ts,
                raw_input: raw.to_string(),
                prompt: prompt.to_string(),
                response: transcript_response,
                usage,
                outcome,
            });
        }
        Ok(false)
    }

    /// debug 模式:任务结束后调用 Debug Agent 评估并落盘报告(失败仅打印,不影响主流程)。
    async fn emit_debug_report(
        &self,
        collector: &Arc<DebugCollector>,
        raw_llm: Arc<dyn crate::llm::LlmClient>,
        task: &str,
        handle_result: &std::result::Result<OrchestrationOutcome, crate::error::AgentError>,
    ) {
        // handle 路径异常(orchestrator 内部错误)时补记终态事件,保证报告完整
        if let Err(e) = handle_result {
            collector.record_task_end(format!("error: {e}"), crate::llm::Usage::default());
        }
        let model = match self.db.lock().expect("db").get_active_or_env() {
            Ok(Some(r)) => format!(
                "[{}] {}/{} @ {}",
                r.protocol.as_str(),
                r.provider_name,
                r.model_name,
                r.end_point
            ),
            _ => "<未配置>".to_string(),
        };
        let meta = ReportMeta {
            mode: "TUI 多轮".to_string(),
            task: task.to_string(),
            model,
        };
        let report_dir = self.paths.root_dir.join("DebugReport");
        match finalize_report(collector, raw_llm, &report_dir, &meta).await {
            Ok(path) => println!(
                "{}",
                pathfmt::fit_line(
                    "  [debug] 报告已生成: ",
                    &pathfmt::display_path(&self.paths, &path),
                    ""
                )
            ),
            Err(e) => eprintln!("  [debug] 报告生成失败: {e}"),
        }
    }

    fn print_assistant_text_with_agent(&mut self, agent_name: &str, text: &str, usage: &crate::llm::Usage) {
        if !text.is_empty() {
            println!();
            println!("  [agent: {agent_name}]");
            self.print_with_optional_highlight(text);
            println!();
        } else {
            println!("  (模型未返回文本)");
        }
        self.print_usage(usage);
        // DirectAnswer 路径:print_usage 内部已 take,但作为防御性清空,
        // 防止某些边界场景(usage 全 0 不进入 println)留下脏值。
        self.task_started_at = None;
    }

    fn print_task_result(
        &mut self,
        result: &crate::agent::orchestrator::TaskResult,
    ) {
        // 2026-09-10 第 27 轮 F05:从 self.task_started_at.take() 取任务开始时间,
        // 拼接到「本次用量」行末尾。take 保证只显示一次,后续命令不带耗时。
        let started = self.task_started_at.take();
        for line in format_task_result(result, &self.paths, started).lines() {
            println!("{line}");
        }
    }

    /// 渲染 diff 输出(供 `/diff` 命令使用)。
    pub(crate) fn print_diff_hunk(&self, hunk: &crate::tui::render::diff::DiffHunk) {
        let rendered = crate::tui::render::diff::render_diff_hunk(hunk);
        for line_spans in rendered {
            print!("  ");
            for span in line_spans {
                let attrs_ansi = attrs_to_ansi(span.attrs);
                // 2026-09-10 第二十五轮 F04/B07 测试修复:输出 bg ANSI 序列,
                // 让 `diff_added_char_bg` / `diff_removed_char_bg` 主题色真正生效(此前死代码)。
                let bg_ansi = bg_color_ansi(span.bg);
                print!(
                    "\x1b[38;5;{}m{}{}{}\x1b[0m",
                    color_to_ansi256(span.fg),
                    bg_ansi,
                    attrs_ansi,
                    span.text
                );
            }
            println!();
        }
    }

    /// 带围栏检测的文本输出:在 ```lang ... ``` 围栏内按语言高亮,围栏外原样输出。
    fn print_with_optional_highlight(&self, text: &str) {
        let mut in_fence = false;
        let mut fence_lang = crate::tui::render::highlight::HlLang::Plain;

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                let tag = trimmed.trim_start_matches('`').trim();
                if in_fence {
                    // 结束围栏
                    println!("  {line}");
                    in_fence = false;
                } else {
                    // 开始围栏
                    fence_lang = crate::tui::render::highlight::lang_from_fence_tag(tag);
                    println!("  {line}");
                    in_fence = true;
                }
                continue;
            }

            if in_fence {
                // 围栏内按语言高亮
                let spans = crate::tui::render::highlight::highlight_line(line, fence_lang);
                print!("  ");
                for span in spans {
                    let attrs_ansi = attrs_to_ansi(span.attrs);
                    // 2026-09-10 第二十五轮 F04/B07 测试修复:同样支持 bg ANSI 输出,
                    // 为后续语法高亮扩展(关键字底色 / 错误标注底色等)留口子。
                    let bg_ansi = bg_color_ansi(span.bg);
                    print!(
                        "\x1b[38;5;{}m{}{}{}\x1b[0m",
                        color_to_ansi256(span.fg),
                        bg_ansi,
                        attrs_ansi,
                        span.text
                    );
                }
                println!();
            } else {
                println!("  {line}");
            }
        }
    }

    fn print_usage(&mut self, usage: &crate::llm::Usage) {
        if usage.input_tokens > 0 || usage.output_tokens > 0 {
            let mut cache = String::new();
            if usage.cache_read_input_tokens > 0 {
                cache.push_str(&format!("  cache_read={}", usage.cache_read_input_tokens));
            }
            if usage.cache_creation_input_tokens > 0 {
                cache.push_str(&format!("  cache_creation={}", usage.cache_creation_input_tokens));
            }
            // 任务总耗时(2026-09-10 第 27 轮 F05 / tmpPlan/2026-09-10_22):
            // 从任务提交到 print_usage 调用的 wall-clock 时间,以秒为单位显示,
            // 便于用户判断 LLM 响应速度;通过 self.task_started_at(任务开始时
            // 在 handle_normal_prompt 入口捕获)计算。读后立即清空,防止下次 print_usage
            // (非任务场景,如 /model /provider list 等)误带耗时。
            let elapsed_str = self
                .task_started_at
                .take()
                .map(|t| format!("  (耗时 {:.2}s)", t.elapsed().as_secs_f64()))
                .unwrap_or_default();
            println!(
                "  本次用量: input={}  output={}{}{}",
                usage.input_tokens, usage.output_tokens, cache, elapsed_str
            );
        } else {
            // 非任务场景(usage 全 0)也清空,防止 /model /provider 等命令误带耗时。
            self.task_started_at = None;
        }
    }
}
