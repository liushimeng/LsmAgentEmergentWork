//! TUI 输入分发与任务结果输出(自 tui/mod.rs 拆分,2026-09-11,单文件 ≤1800 行规范)。
//!
//! 职责:`handle_user_input` 入口分流(`?` 别名 / 斜杠命令 / 普通提示词)→
//! `dispatch_prompt` 统一任务分发(@ 提及展开 / 阶段进度协程 / SIGINT 取消 /
//! debug 报告 / 终态打印 / transcript 记录),以及结果渲染 `print_*` 家族。

use std::io::Write;
use std::sync::Arc;

use anyhow::Result;

use super::export::{OutcomeKind, TranscriptEntry};
use super::format::{
    format_task_result, format_task_result_for_context, merge_usage, now_clock, waiting_line_text,
};
use super::pathfmt;
use super::TuiSession;
use crate::agent::debug::{finalize_report, DebugCollector, ReportMeta};
use crate::agent::orchestrator::OrchestrationOutcome;
use crate::llm::{ChatMessage, Usage};

impl TuiSession {
    pub async fn handle_user_input(&mut self, line: &str) -> Result<bool> {
        // 启动期恢复(第 96 轮,--resume / -c):main 在 TUI 启动前写入静态请求,
        // 此处(所有输入的统一入口,先于斜杠/提示词分流)消费一次。经静态量传递而非
        // 改 run_with_debug 签名,规避对并行任务占用中的 tui/mod.rs 的改动。
        if let Some(spec) = crate::database::chat_store::take_startup_resume() {
            self.apply_startup_resume(&spec);
        }
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
        // 未配置 Provider 前置守门(2026-09-17 第 71 轮,tmpPlan/2026-09-17_03):
        // 无 active 接入记录时,orchestrator 挂的是 NoopLlm —— 占位文本会被当成
        // "成功的 LLM 响应" 送进全链路:Yolo 解析失败降级 → SubAgent 零工具空转 →
        // QC fail-closed「Quality 报告解析失败」→ 盲重试 3 轮,最终以与根因完全
        // 无关的误导性错误收口。与 -p 单轮模式(main.rs run_one_shot)对齐:fail fast,
        // 不 push session 上下文、不进编排器、零 LLM 调用、零重试、不动连接状态。
        // 放在 D13 离线检查之前:配置缺失是本地 DB 状态而非网络问题,入队毫无意义。
        match self.db.lock().expect("db").get_active_or_env() {
            Ok(Some(_)) => {}
            Ok(None) => {
                println!();
                println!("  [未配置] 尚未配置大模型接入记录,本条任务未执行(未进入编排器)。");
                println!("  [未配置] 请先在 TUI 内执行 /provider add 添加接入记录,");
                println!("  [未配置] 或使用 CLI: laew provider add …;配置完成后重新发送提示词。");
                return Ok(false);
            }
            Err(e) => {
                // LAEW_PROVIDER_ID 指向不存在记录等配置错误:错误信息本身含修复指引
                eprintln!("  [未配置] 读取接入记录失败: {e}");
                eprintln!("  [未配置] 可用 /provider list 检查接入记录,或 /provider add 新增。");
                return Ok(false);
            }
        }
        // D13 离线模式(2026-09-11):
        // - Offline → 入队跳过 LLM 调用(避免浪费 30s 重试链),等恢复后 flush;
        // - Online/Degraded 且有积压队列 → 本条用户输入触发 flush 一条队列头部
        //   (逐条 flush 避免递归 dispatch 导致顺序错乱,用户当前提示词顺延到下一笔)。
        let queued_req = if !self.connectivity.should_attempt_llm() {
            // 离线:当前提示词入队
            return self.enqueue_offline(raw, prompt).await;
        } else if !self.offline_queue.is_empty() {
            // 恢复:flush 一条队列头部(如有)
            self.offline_queue.drain().into_iter().next()
        } else {
            None
        };
        let (effective_raw, effective_prompt, is_flush) = match &queued_req {
            Some(req) => {
                println!(
                    "  [离线] 恢复连接,flush 队列第 1 条(剩余 {} 条)",
                    self.offline_queue.len()
                );
                (req.raw.as_str(), req.prompt.as_str(), true)
            }
            None => (raw, prompt, false),
        };
        // 普通提示词:Orchestrator 编排(可取消:Ctrl-C 经 SIGINT 自动感知,零新增命令)
        self.session
            .context_mut()
            .push(ChatMessage::user(effective_prompt));
        // debug 模式:每个任务开始前重置采集器
        if let Some(collector) = &self.debug {
            collector.reset(self.session.id());
        }
        // D4 工作区感知(2026-09-13 第 01 轮):任务执行前取一次轻量变更标记,
        // 任务结束后再取一次,自动向用户呈现「本次任务让工作区变化了什么」。
        // 非 git 目录返回 is_git=false,对比直接跳过(零噪音)。
        let ws_before = crate::agent::workspace::change_marker(&self.paths.work_dir);
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
            let mut spinner_started_at = std::time::Instant::now();
            // 2026-09-16 第 59 轮:阶段计时 —— 每条 stage 消息的到达时间,
            // 用于计算与上一条 stage 的时间差(Δ)。
            let mut last_stage_at: Option<std::time::Instant> = None;
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
            // 第 100 轮(HITL):人工介入轮询 —— MCP_Web_Use request_human 时,
            // HumanAssistHub 槽位出现待答请求,本协程渲染请求块并阻塞读 stdin。
            // 任务执行期终端处于 cooked mode(InputHandler 未占用),行读即回显;
            // 空回车=选项1,数字=对应选项,q/取消=取消(→工具 4002),其余原文返回。
            let mut assist_interval =
                tokio::time::interval(std::time::Duration::from_millis(250));
            assist_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut handled_assist_id: Option<u64> = None;

            loop {
                tokio::select! {
                    maybe = stage_rx.recv() => match maybe {
                        Some(line) => {
                            // 不在此处清除 waiting 行:清除统一延迟到 idle flush,
                            // 否则「recv 清行 → 心跳用过期 current_stage 复活旧行 →
                            // flush println 落在行中部」的竞态会复发行中部错位
                            // (心跳分支另有 queue.is_empty() 闸门双保险)。
                            //
                            // F3 补(2026-09-14 第 51 轮):`[laew]` 前缀的重要通知
                            // (Context 压缩 / WorkFlow 并行调度)不走 1.5s hold ——
                            // 立即冲刷,且任务快速完成时也不得被丢弃
                            // (e2e 5c mock 场景 hold 窗口内结束曾把压缩提示整条吞掉)。
                            if line.starts_with("[laew]") {
                                if waiting_line_on_screen {
                                    clear_waiting_line(stdout_is_tty);
                                    waiting_line_on_screen = false;
                                    initial_spinner_active = false;
                                }
                                // 先冲刷积压的普通阶段,保持时序
                                while let Some(pending) = queue.pop_front() {
                                    // 2026-09-16 第 59 轮:积压阶段也带时间
                                    let timing = if let Some(last) = last_stage_at {
                                        let delta_ms = last.elapsed().as_millis();
                                        if delta_ms < 1000 {
                                            format!(" +{delta_ms}ms")
                                        } else {
                                            format!(" +{:.1}s", delta_ms as f64 / 1000.0)
                                        }
                                    } else {
                                        String::new()
                                    };
                                    println!("  [stage] {pending}{timing}");
                                    last_stage_at = Some(std::time::Instant::now());
                                    // 2026-09-18 第 88 轮:即时冲刷分支也要推进 current_stage,
                                    // 否则 waiting 心跳一直显示更早的旧阶段名
                                    // (实测 SubAgent 执行 550s,spinner 仍显示「Main-Work 拆解中…」)。
                                    current_stage = Some((
                                        short_stage_label(&pending),
                                        std::time::Instant::now(),
                                    ));
                                }
                                let timing = if let Some(last) = last_stage_at {
                                    let delta_ms = last.elapsed().as_millis();
                                    if delta_ms < 1000 {
                                        format!(" +{delta_ms}ms")
                                    } else {
                                        format!(" +{:.1}s", delta_ms as f64 / 1000.0)
                                    }
                                } else {
                                    String::new()
                                };
                                println!("  [stage] {line}{timing}");
                                let _ = std::io::stdout().flush();
                                last_stage_at = Some(std::time::Instant::now());
                                idle.as_mut().reset(tokio::time::Instant::now() + tick);
                                continue;
                            }
                            if queue.is_empty() {
                                idle.as_mut().reset(tokio::time::Instant::now() + hold);
                            }
                            queue.push_back(line);
                        }
                        None => {
                            // 任务结束:清除屏幕上的 waiting 行,丢弃未显示的快速阶段;
                            // 但 [laew] 重要通知不允许丢弃(F3 补,同上)。
                            while let Some(pending) = queue.pop_front() {
                                if pending.starts_with("[laew]") {
                                    if waiting_line_on_screen {
                                        clear_waiting_line(stdout_is_tty);
                                        waiting_line_on_screen = false;
                                    }
                                    // 2026-09-16 第 59 轮:任务结束时的 [laew] 也带时间
                                    let timing = if let Some(last) = last_stage_at {
                                        let delta_ms = last.elapsed().as_millis();
                                        if delta_ms < 1000 {
                                            format!(" +{delta_ms}ms")
                                        } else {
                                            format!(" +{:.1}s", delta_ms as f64 / 1000.0)
                                        }
                                    } else {
                                        String::new()
                                    };
                                    println!("  [stage] {pending}{timing}");
                                    let _ = std::io::stdout().flush();
                                }
                            }
                            if waiting_line_on_screen {
                                clear_waiting_line(stdout_is_tty);
                            }
                            break;
                        }
                    },
                    _ = assist_interval.tick() => {
                        if let Some(req) =
                            crate::agent::human_assist::HumanAssistHub::global().poll()
                        {
                            if handled_assist_id != Some(req.id) {
                                handled_assist_id = Some(req.id);
                                if waiting_line_on_screen {
                                    clear_waiting_line(stdout_is_tty);
                                    waiting_line_on_screen = false;
                                }
                                initial_spinner_active = false;
                                print_human_assist_block(&req);
                                let answer = read_human_assist_answer().await;
                                let mapped =
                                    map_human_assist_input(&answer, &req.options);
                                let ok = crate::agent::human_assist::HumanAssistHub::global()
                                    .respond(req.id, mapped.clone());
                                if ok {
                                    match mapped {
                                        Some(a) => {
                                            println!("  [laew] 已收到人工输入:{a}")
                                        }
                                        None => println!(
                                            "  [laew] 人工已取消介入,任务按取消路径继续"
                                        ),
                                    }
                                } else {
                                    println!("  [laew] 该人工介入请求已失效(可能已超时)");
                                }
                                // 恢复 spinner 计时,避免把等待人工的时间算进阶段耗时
                                spinner_started_at = std::time::Instant::now();
                            }
                        }
                    }
                    _ = &mut idle => {
                        // 阶段切换 flush:第一批 stage 行打印前先清 waiting 行
                        // (统一 \r\x1b[K,从列 0 打印,不吞历史行)。
                        while let Some(line) = queue.pop_front() {
                            if waiting_line_on_screen {
                                clear_waiting_line(stdout_is_tty);
                                waiting_line_on_screen = false;
                                initial_spinner_active = false;
                            }
                            // 2026-09-16 第 59 轮:计算与上一条 stage 的时间差
                            let timing = if let Some(last) = last_stage_at {
                                let delta_ms = last.elapsed().as_millis();
                                if delta_ms < 1000 {
                                    format!(" +{delta_ms}ms")
                                } else {
                                    format!(" +{:.1}s", delta_ms as f64 / 1000.0)
                                }
                            } else {
                                String::new()
                            };
                            println!("  [stage] {line}{timing}");
                            let _ = std::io::stdout().flush();
                            last_stage_at = Some(std::time::Instant::now());
                            // 2026-09-16 第 62 轮:current_stage 只保留短标题,
                            // 避免 waiting 心跳每 1s 把超长 stage 文本原地重写。
                            // 超长详情走 [laew] 前缀立即冲刷,不进 waiting 流。
                            current_stage = Some((short_stage_label(&line), std::time::Instant::now()));
                        }
                        // 阶段内 waiting 心跳:每 1s 重写一行(覆盖上一帧)。
                        // queue 非空(有待冲刷阶段)时绝不重写 —— 防止用旧
                        // current_stage 把已清掉的行复活,导致下一条 [stage] 错位。
                        if queue.is_empty() {
                            let (text, next) =
                                if let Some((stage, started)) = &current_stage {
                                    // 2026-09-16 第 66 轮:阶段超时提示 ——
                                    // 30s 提示「耗时较长」,60s 提示「耗时过长,Ctrl-C 取消」,
                                    // 让用户感知进度而非「卡住无响应」。
                                    let elapsed = started.elapsed().as_secs();
                                    if elapsed == 30 {
                                        let _ = std::io::stdout().write_all(
                                            format!(
                                                "\r  [laew] 阶段「{stage}」耗时较长,请稍候... ({elapsed}s)\x1b[K\n"
                                            )
                                            .as_bytes(),
                                        );
                                        let _ = std::io::stdout().flush();
                                    } else if elapsed == 60 {
                                        let _ = std::io::stdout().write_all(
                                            format!(
                                                "\r  [laew] 阶段「{stage}」耗时过长,Ctrl-C 取消 ({elapsed}s)\x1b[K\n"
                                            )
                                            .as_bytes(),
                                        );
                                        let _ = std::io::stdout().flush();
                                    }
                                    (
                                        waiting_line_text(
                                            Some(stage),
                                            SPINNER[spinner_idx % SPINNER.len()],
                                            elapsed,
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
        // 任务窗口 SIGINT 监听(第 101 轮优化):第一次 Ctrl+C 立即取消 + exit(130),
        // 与 main.rs 单轮模式行为一致。旧实现「第一次取消、等第二次才 exit」在
        // crossterm 原始模式下 tokio::signal::ctrl_c() 可能无法再次触发,导致进程
        // 被用户 Ctrl-Z suspend 而非正常退出。
        let sig_cancel = cancel.clone();
        let sig_task = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            eprintln!();
            eprintln!("  [laew] 收到中断信号,正在取消当前任务...");
            sig_cancel.cancel();
            // 给取消传播 300ms 窗口,然后强制退出(不等第二次 Ctrl-C)。
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            eprintln!("  [laew] 任务已取消(用户中断)");
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
        // 第 100 轮(HITL)兜底:任务结束时丢弃未应答的人工介入请求,
        // 防止挂起请求跨任务泄漏(工具侧收到 Unavailable → 4001 语义)。
        crate::agent::human_assist::HumanAssistHub::global().cancel_pending();
        // debug 模式:任务结束(无论成败)后生成 Debug 报告
        if let (Some(collector), Some(raw_llm)) = (&self.debug, &self.debug_llm_raw) {
            self.emit_debug_report(collector, raw_llm.clone(), effective_prompt, &handle_result)
                .await;
        }
        // D13:任务结束更新连接状态(基于最终结果)。
        self.update_connectivity_from_result(&handle_result, is_flush);
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
                // 2026-09-15 TUIMarkdown 富文本渲染:transcript 走 styled=false 纯文本
                // (导出/回填零 ANSI),屏幕打印走 styled=true(仅模型内容块着色)。
                let transcript_text = format_task_result(
                    result,
                    &self.paths,
                    self.task_started_at,
                    false,
                    self.task_cost_hint(&result.total_usage).as_deref(),
                );
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
                last_trace,
                stage_durations,
                retry_log,
                wallclock_ms,
            }) => {
                println!();
                // 2026-09-16 第 58 轮 P0-D:F4 头部呈现真实失败原因 + 总耗时
                // (此前只有 suggestion,可能与真实失败无关而误导用户)。
                let reason_short = if reason.is_empty() {
                    "(未提供失败原因)".to_string()
                } else {
                    reason.clone()
                };
                println!(
                    "  [task failed: difficulty={}, reason={}, 总耗时 {:.2}s]",
                    classification.task_level.display_name(),
                    crate::tui::format::truncate_chars(&reason_short, 240),
                    *wallclock_ms as f64 / 1000.0,
                );
                // 2026-09-16 第 64 轮(第 89 轮更新):浏览器类任务失败时额外打印诊断。
                // Chromium-WebUse Agent 已删除,浏览器操控由 SubAgent-Work 的 MCP_Web_Use
                // 工具承担;tool_calls=0 的排查方向相应更新。
                if reason.contains("tool_calls=0") || reason.contains("no_tool_use") {
                    println!("  [浏览器任务失败诊断] 详见下方 [trace]/[tool]/[failure] 段;若 tool_calls=0 排查方向:1) Chrome/Edge/Chromium 安装(MCP_Web_Use 返回 code=3001 时如实告知用户);2) 设置 LAEW_BROWSER_PATH 指向浏览器可执行文件。");
                }
                // 2026-09-17 第 75 轮:trace 携带 delegate_mismatch 弱信号时,直接打印
                // 「路由错配」诊断行,说明 WorkFlow 期望的角色与 Runner 实际执行的角色
                // 不一致。让用户/QA 一眼看出问题根因,不必翻 Debug 报告
                // (第 89 轮起执行器统一 SubAgent,该信号保留供未来执行器扩展对账)。
                let mismatch_info: Option<(String, String)> = last_trace
                    .as_ref()
                    .and_then(|t| {
                        let sig = t.failure_signals.iter().find(|s| s.starts_with("delegate_mismatch:"))?;
                        let runner = t.runner_role.map(|r| r.as_str().to_string()).unwrap_or_default();
                        let intended = t.intended_role.map(|r| r.as_str().to_string()).unwrap_or_default();
                        // 取掉信号前缀,展示简洁对
                        let _ = sig;
                        Some((runner, intended))
                    });
                if let Some((runner, intended)) = mismatch_info {
                    println!(
                        "  [路由错配] Runner={runner} 但 WorkFlow 期望={intended};\n    ↳ Runner 的工具集与任务需求不匹配,典型场景:网页任务 → SubAgentRunner(无 Browser* 工具)。\n    ↳ 排查:1) Main-Work delegate 推断是否被 infer_delegate_to 正确覆盖;2) 提交 issue 时附 [trace] 段 runner_role/intended_role。"
                    );
                }
                // 复用 format_task_result 渲染 stage_durations / retry_log / trace 段。
                // stub_workflow 用 last_trace 携带失败单元的工具调用明细,便于 [trace] [tool] [failure] 段呈现。
                let stub_workflow =
                    last_trace
                        .as_ref()
                        .map(|t| crate::agent::orchestrator::WorkflowResult {
                            id: "wf-1".into(),
                            name: "失败单元".into(),
                            subflow_outcome: String::new(),
                            quality_report: crate::agent::quality::QualityReport {
                                verdict: crate::agent::quality::Verdict::Fail,
                                issues: vec![reason.clone()],
                                suggestion: suggestion.clone(),
                                retryable: false,
                                source: crate::agent::context::AgentRole::SubAgent,
                                evidence: String::new(),
                            },
                            usage: Usage::default(),
                            subflow_trace: Some(t.as_ref().clone()),
                            exec_role: crate::agent::context::AgentRole::SubAgent,
                            wallclock_ms: 0,
                            qc_wallclock_ms: 0,
                        });
                let stub_result = crate::agent::orchestrator::TaskResult {
                    goal: classification.goal_summary.clone(),
                    classification: classification.clone(),
                    plan_doc: None,
                    workflows: stub_workflow.into_iter().collect(),
                    summary: String::new(),
                    total_usage: *usage,
                    stage_durations: stage_durations.clone(),
                    retry_log: retry_log.clone(),
                    layer_log: vec![],
                    wallclock_ms: *wallclock_ms,
                };
                // 复用 format_failed_detail 渲染 stage_durations / retry_log / 失败工具明细 / failure_signals,
                // 不再走 format_task_result(其会打印 "[task executed]" 与失败语义冲突)。
                for line in
                    crate::tui::format::format_failed_detail(&stub_result, &reason, &suggestion)
                        .lines()
                {
                    println!("{line}");
                }
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
                Some((OutcomeKind::Failed, text.clone(), text, *usage))
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
            // D4 工作区感知(2026-09-13 第 01 轮):任务结束后的工作区变更提示。
            self.print_workspace_delta(&ws_before);
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
                raw_input: effective_raw.to_string(),
                prompt: effective_prompt.to_string(),
                response: transcript_response,
                // 会话持久化(第 96 轮):上下文回填版单独留存,恢复重建 context 时
                // 优先取此字段(Executed 轮与人类版分流,第 30 轮语义)。
                context_response: Some(context_response),
                usage,
                // D8 成本(2026-09-17 第 76 轮):按当时 active 模型内置价估算;
                // 零用量(取消/错误)与无价模型记 None,不污染会话合计。
                cost_usd: self.estimate_entry_cost(&usage),
                outcome,
            });
            // 会话持久化(第 96 轮,2026-09-19):每轮收口整快照落盘,零配置;
            // 失败仅告警不影响对话(方案 tmpPlan/2026-09-19_02)。
            self.persist_chat_history();
        }
        // 第 103 轮:返回前强制冲刷 stdout,确保 dispatch_prompt 内所有异步阶段输出
        // (stage_printer 协程的 println!) 已全部落盘,再交还主循环进入 read_line。
        // 避免「输出未完成就重绘输入面板」的竞态(焦点丢失的根因之一)。
        let _ = std::io::stdout().flush();
        Ok(false)
    }

    /// 会话持久化(第 96 轮):把当前 transcript 整快照写入根目录 SQLite
    /// `chat_sessions`/`chat_turns`,供 `/sessions` `/resume` 与 `--resume` 跨进程恢复。
    ///
    /// - 标题 = 首轮 raw_input 首行预览(≤40 字符),只在会话行首次插入生效;
    /// - `context_response` 缺失(DirectAnswer 等两版相同)回退 `response`;
    /// - 失败语义:**绝不打断对话**,tracing::warn 落 {根目录}/logs/laew-tui.log。
    pub(crate) fn persist_chat_history(&self) {
        if self.transcript.is_empty() {
            return;
        }
        let turns: Vec<crate::database::chat_store::ChatTurnRow> = self
            .transcript
            .iter()
            .enumerate()
            .map(|(i, e)| crate::database::chat_store::ChatTurnRow {
                seq: i as i64 + 1,
                ts: e.ts.clone(),
                raw_input: e.raw_input.clone(),
                prompt: e.prompt.clone(),
                response: e.response.clone(),
                context_response: e
                    .context_response
                    .clone()
                    .unwrap_or_else(|| e.response.clone()),
                outcome: e.outcome.as_store_str().to_string(),
                usage: e.usage,
                cost_usd: e.cost_usd,
            })
            .collect();
        let title = super::format::first_line_preview(&self.transcript[0].raw_input, 40);
        let work_dir = self.paths.work_dir.display().to_string();
        if let Err(e) = self.db.lock().expect("db").save_chat_snapshot(
            &self.session.id,
            &self.session.created_at,
            &work_dir,
            self.current_model_name().as_deref(),
            &title,
            &turns,
        ) {
            tracing::warn!(session_id = %self.session.id, error = %e, "会话持久化失败(不影响当前对话)");
        }
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
        // Round 92: extract classification from handle_result for Debug Agent activation control.
        let classification = match handle_result {
            Ok(crate::agent::orchestrator::OrchestrationOutcome::DirectAnswer { classification, .. })
            | Ok(crate::agent::orchestrator::OrchestrationOutcome::Failed { classification, .. }) => {
                Some(classification.clone())
            }
            Ok(crate::agent::orchestrator::OrchestrationOutcome::Executed { result }) => {
                Some(result.classification.clone())
            }
            Err(_) => None,
        };
        let meta = ReportMeta {
            mode: "TUI 多轮".to_string(),
            task: task.to_string(),
            model,
            classification,
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

    fn print_assistant_text_with_agent(
        &mut self,
        agent_name: &str,
        text: &str,
        usage: &crate::llm::Usage,
    ) {
        if !text.is_empty() {
            println!();
            println!("  [agent: {agent_name}]");
            self.print_assistant_markdown(text);
            println!();
        } else {
            println!("  (模型未返回文本)");
        }
        self.print_usage(usage);
        // DirectAnswer 路径:print_usage 内部已 take,但作为防御性清空,
        // 防止某些边界场景(usage 全 0 不进入 println)留下脏值。
        self.task_started_at = None;
    }

    fn print_task_result(&mut self, result: &crate::agent::orchestrator::TaskResult) {
        // 2026-09-10 第 27 轮 F05:从 self.task_started_at.take() 取任务开始时间,
        // 拼接到「本次用量」行末尾。take 保证只显示一次,后续命令不带耗时。
        let started = self.task_started_at.take();
        // styled=true:仅模型内容块(subflow_outcome/摘要)经 Markdown 渲染加 ANSI,
        // 结构标签行保持纯文本(2026-09-15,docs/TUIMarkdown富文本渲染/)。
        for line in format_task_result(
            result,
            &self.paths,
            started,
            true,
            self.task_cost_hint(&result.total_usage).as_deref(),
        )
        .lines()
        {
            println!("{line}");
        }
    }

    /// 渲染 diff 输出(供 `/diff` 命令使用)。
    pub(crate) fn print_diff_hunk(&self, hunk: &crate::tui::render::diff::DiffHunk) {
        // ANSI 编码收敛到 render::span_to_ansi(2026-09-15 TUIMarkdown 富文本渲染);
        // 2026-09-10 第二十五轮 F04/B07 的 bg 序列语义保持(diff 字符级底色高亮生效)。
        let rendered = crate::tui::render::diff::render_diff_hunk(hunk);
        crate::tui::render::print_render_lines(&rendered, "  ");
    }

    /// 助手文本输出(2026-09-15 TUIMarkdown 富文本渲染):整段走 Markdown 渲染器。
    ///
    /// 旧实现只在 ```lang 围栏内做语法高亮、围栏外原样输出;现由
    /// `render::markdown::print_markdown` 统一着色(标题/粗斜体/行内码/列表/
    /// 引用/表格/链接/分隔线),围栏内语法高亮语义内嵌保留(行为向后兼容)。
    /// 设计见 docs/TUIMarkdown富文本渲染/01-设计与解决方案.md。
    fn print_assistant_markdown(&self, text: &str) {
        crate::tui::render::markdown::print_markdown(text, "  ");
    }

    // ========================================================================
    // D8 会话成本估算(2026-09-17 第 76 轮)
    // ========================================================================

    /// 当前 active 模型的 model_name;未配置/查询失败 → None。
    pub(crate) fn current_model_name(&self) -> Option<String> {
        self.db
            .lock()
            .expect("db")
            .get_active_or_env()
            .ok()
            .flatten()
            .map(|r| r.model_name)
    }

    /// 估算单轮用量成本(USD):零用量(取消/错误轮)与无价模型 → None。
    fn estimate_entry_cost(&self, usage: &crate::llm::Usage) -> Option<f64> {
        if usage.input_tokens == 0 && usage.output_tokens == 0 {
            return None;
        }
        let model = self.current_model_name()?;
        crate::llm::estimate_cost_usd(&model, usage)
    }

    /// 会话成本汇总:从 transcript 逐轮实记 fold(rewind/switch/clear 后自动一致)。
    /// 返回 (已计价合计 USD, 是否存在无价轮次)。
    pub(crate) fn session_cost_summary(&self) -> (f64, bool) {
        let mut total = 0.0;
        let mut partial = false;
        for e in &self.transcript {
            match e.cost_usd {
                Some(c) => total += c,
                None => {
                    if e.usage.input_tokens > 0 || e.usage.output_tokens > 0 {
                        partial = true;
                    }
                }
            }
        }
        (total, partial)
    }

    /// 用量行的成本提示串(`$X.XXXX` 形态);无价/零用量 → None。
    fn task_cost_hint(&self, usage: &crate::llm::Usage) -> Option<String> {
        self.estimate_entry_cost(usage)
            .map(|c| crate::llm::format_usd(c))
    }

    fn print_usage(&mut self, usage: &crate::llm::Usage) {
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
            // 从任务提交到 print_usage 调用的 wall-clock 时间,以秒为单位显示,
            // 便于用户判断 LLM 响应速度;通过 self.task_started_at(任务开始时
            // 在 handle_normal_prompt 入口捕获)计算。读后立即清空,防止下次 print_usage
            // (非任务场景,如 /model /provider list 等)误带耗时。
            let elapsed_str = self
                .task_started_at
                .take()
                .map(|t| format!("  (耗时 {:.2}s)", t.elapsed().as_secs_f64()))
                .unwrap_or_default();
            // D8 成本(2026-09-17 第 76 轮):模型有内置参考价时行尾追加估算成本;
            // 无价/未配置时不追加,行格式与旧版一致。
            let cost_str = self
                .estimate_entry_cost(usage)
                .map(|c| format!("  成本≈{}", crate::llm::format_usd(c)))
                .unwrap_or_default();
            println!(
                "  本次用量: input={}  output={}{}{}{}",
                usage.input_tokens, usage.output_tokens, cache, elapsed_str, cost_str
            );
        } else {
            // 非任务场景(usage 全 0)也清空,防止 /model /provider 等命令误带耗时。
            self.task_started_at = None;
        }
    }

    // ========================================================================
    // D13 离线模式(2026-09-11):离线入队 / 连接状态更新
    // ========================================================================

    /// 离线时将当前提示词入队,返回 Ok(false) 提示用户。
    async fn enqueue_offline(&self, raw: &str, prompt: &str) -> Result<bool> {
        match self
            .offline_queue
            .enqueue(prompt.to_string(), raw.to_string())
        {
            Ok(()) => {
                println!(
                    "  [离线] Provider 不可达,已排队 #{} (恢复后自动处理)",
                    self.offline_queue.len()
                );
                Ok(false)
            }
            Err(crate::agent::offline_queue::QueueFull(cap)) => {
                eprintln!("  [离线] 队列已满({cap}),请稍后再试");
                Ok(false)
            }
        }
    }

    /// 任务结束后更新连接状态(被动检测核心逻辑)。
    fn update_connectivity_from_result(
        &self,
        result: &std::result::Result<OrchestrationOutcome, crate::error::AgentError>,
        is_flush: bool,
    ) {
        match result {
            Ok(OrchestrationOutcome::DirectAnswer { .. })
            | Ok(OrchestrationOutcome::Executed { .. }) => {
                // 任务成功 → 网络可达,复位连接状态。
                self.connectivity.record_success();
            }
            Ok(OrchestrationOutcome::Failed { reason, .. }) => {
                // 编排层失败:检查 reason 是否网络类。
                if looks_like_network_failure(reason) {
                    self.connectivity
                        .record_network_error(categorize_failure_reason(reason));
                } else {
                    // 非网络失败(如 Yolo 解析失败 / 任务逻辑失败)→ 网络可达。
                    self.connectivity.record_success();
                }
            }
            Err(e) => match e {
                crate::error::AgentError::LlmNetwork(_) => {
                    self.connectivity.record_network_error("llm_network");
                }
                crate::error::AgentError::LlmHttp { status, .. } => {
                    // 5xx 视为 Provider 不可达;4xx(除 429/408)视为服务可达。
                    if *status == 502 || *status == 503 || *status == 504 || *status == 429 {
                        self.connectivity
                            .record_network_error(format!("http_{status}"));
                    } else {
                        // 4xx 其他(401/400/422)→ 服务可达,网络正常。
                        self.connectivity.record_success();
                    }
                }
                crate::error::AgentError::Cancelled => {
                    // 取消不影响连接状态。
                }
                // 其他错误(Yolo 解析失败等)→ 服务可达。
                _ => {
                    self.connectivity.record_success();
                }
            },
        }
        // D13:如果是 flush 队列的任务,记录恢复探测日志。
        if is_flush {
            let state = self.connectivity.state();
            match state {
                crate::llm::Connectivity::Online => {
                    // 静默:恢复成功无需提示(横幅会显示 Online)。
                }
                crate::llm::Connectivity::Degraded | crate::llm::Connectivity::Offline => {
                    println!("  [离线] 探测仍失败,保持离线状态");
                }
            }
        }
    }

    /// D4 工作区感知(2026-09-13 第 01 轮):任务执行后的工作区变更提示。
    ///
    /// 只在「用户可感知的变化」出现时打印:分支切换 / 未提交变更数变化 /
    /// 新增变更文件。非 git 目录或无变化时静默 —— 避免每轮任务刷屏。
    /// 同时失效工作区缓存,让下一轮注入的运行时 brief 反映任务后的真实状态。
    fn print_workspace_delta(&self, before: &crate::agent::workspace::WorkspaceDelta) {
        crate::agent::workspace::invalidate();
        if !before.is_git {
            return;
        }
        let after = crate::agent::workspace::change_marker(&self.paths.work_dir);
        if !after.is_git {
            return;
        }
        let branch_changed = before.branch != after.branch;
        let new_files: Vec<&String> = after
            .files
            .iter()
            .filter(|f| !before.files.contains(f))
            .collect();
        if !branch_changed && after.dirty == before.dirty && new_files.is_empty() {
            return;
        }
        if branch_changed {
            println!(
                "  [工作区] 分支 {} → {}",
                before.branch.as_deref().unwrap_or("?"),
                after.branch.as_deref().unwrap_or("?")
            );
        }
        if after.dirty != before.dirty {
            let delta = after.dirty as isize - before.dirty as isize;
            let sign = if delta > 0 { "+" } else { "" };
            print!(
                "  [工作区] 未提交变更 {} → {}({sign}{delta})",
                before.dirty, after.dirty
            );
        } else {
            print!("  [工作区] 变更文件集变化(共 {} 个未提交)", after.dirty);
        }
        if !new_files.is_empty() {
            let shown: Vec<&str> = new_files.iter().take(3).map(|f| f.as_str()).collect();
            let more = if new_files.len() > shown.len() {
                format!(" 等 {} 个", new_files.len())
            } else {
                String::new()
            };
            print!(": {}{more}", shown.join(", "));
        }
        println!();
    }
}

/// 2026-09-16 第 63 轮(升级):把 stage 文本压成短标题,waiting 心跳不再复读超长文本。
/// - 长描述(单元详情 / QC 详情)走 `[laew]` 前缀立即冲刷,不进入 waiting 流;
/// - 短标题(单元 ID + 执行者)进入 stage 流 + waiting 心跳,每 1s 原地重写时只刷新 spinner。
/// 2026-09-16 第 63 轮:从 60 → 40 字符,避免 waiting 行过长导致重复感。
fn short_stage_label(line: &str) -> String {
    const MAX_CHARS: usize = 40;
    let clean = line.replace(['\n', '\r'], " ");
    if clean.chars().count() <= MAX_CHARS {
        clean
    } else {
        clean
            .chars()
            .take(MAX_CHARS.saturating_sub(1))
            .chain(['…'])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_stage_label_truncates_long_input() {
        let long = "wf-1.step SubAgent 执行中 | 职责: 微信通讯录查找用户并发送AI消息 | 期望: 微信进程存在且窗口可访问; 通讯录界面成功打开";
        let label = short_stage_label(long);
        assert!(label.chars().count() <= 40, "短标题应 ≤ 40 字符:{} 字符", label.chars().count());
        assert!(label.ends_with('…'), "超长应加 …");
    }

    #[test]
    fn short_stage_label_keeps_short_input_intact() {
        let short = "Yolo 分类中";
        let label = short_stage_label(short);
        assert_eq!(label, short);
    }

    #[test]
    fn short_stage_label_strips_newlines() {
        let label = short_stage_label("a\nb\rc");
        assert_eq!(label, "a b c");
    }
}

// =================== 第 100 轮:人工介入(HITL)TUI 渲染与输入 ===================

use crate::agent::human_assist::HumanAssistDisplay;

/// kind 展示标签(request_human 的 reason 映射;与工具侧 human_assist_defaults 对齐)。
fn human_assist_kind_label(kind: &str) -> &str {
    match kind {
        "captcha" => "图形/滑块验证码",
        "sms" => "短信验证码",
        "qr_login" => "扫码登录",
        "login" => "账密登录",
        "manual_verify" => "人工核验",
        _ => "人工介入",
    }
}

/// 渲染人工介入请求块(蓝色框线,与浏览器窗口蓝框呼应)。
fn print_human_assist_block(req: &HumanAssistDisplay) {
    use std::io::Write as _;
    let mut out = String::new();
    out.push_str("\n  \x1b[36m┌─ 🔐 人工介入请求 ────────────────────────────────\x1b[0m\n");
    out.push_str(&format!(
        "  \x1b[36m│\x1b[0m 类型: \x1b[1m{}\x1b[0m",
        human_assist_kind_label(&req.kind)
    ));
    if !req.url.is_empty() {
        out.push_str(&format!("   页面: {}", pathfmt::elide_middle(&req.url, 56)));
    }
    out.push('\n');
    out.push_str(&format!(
        "  \x1b[36m│\x1b[0m 说明: {}\n",
        req.message.replace('\n', " ")
    ));
    if !req.options.is_empty() {
        out.push_str("  \x1b[36m│\x1b[0m 选项:\n");
        for (i, opt) in req.options.iter().enumerate() {
            out.push_str(&format!("  \x1b[36m│\x1b[0m   {}. {}\n", i + 1, opt));
        }
    }
    out.push_str(&format!(
        "  \x1b[36m│\x1b[0m 超时: {}s\n",
        req.timeout_ms / 1000
    ));
    out.push_str("  \x1b[36m└──────────────────────────────────────────────\x1b[0m\n");
    let prompt_line = if req.options.is_empty() {
        "  请输入内容后回车(q=取消): "
    } else {
        "  请输入选项编号或直接输入内容(回车=1,q=取消): "
    };
    out.push_str(prompt_line);
    print!("{out}");
    let _ = std::io::stdout().flush();
}

/// 阻塞读一行 stdin(spawn_blocking 防 tokio 协程阻塞;cooked mode 自带回显)。
/// EOF / 读取失败 → 空串(按默认选项 1 处理,避免 e2e 管道场景挂死)。
async fn read_human_assist_answer() -> String {
    tokio::task::spawn_blocking(|| {
        let mut buf = String::new();
        match std::io::stdin().read_line(&mut buf) {
            Ok(0) | Err(_) => String::new(),
            Ok(_) => buf.trim_end_matches(['\n', '\r']).to_string(),
        }
    })
    .await
    .unwrap_or_default()
}

/// 人工输入映射:空回车 → 选项 1;数字 → 对应选项;q/取消/cancel → None(取消);
/// 其余原文返回(短信验证码数字即自由文本路径)。
fn map_human_assist_input(input: &str, options: &[String]) -> Option<String> {
    let t = input.trim();
    if t.is_empty() {
        return options.first().map(|o| format!("1. {o}"));
    }
    let lower = t.to_lowercase();
    if matches!(lower.as_str(), "q" | "quit" | "取消" | "cancel" | "exit") {
        return None;
    }
    if let Ok(n) = t.parse::<usize>() {
        if (1..=options.len()).contains(&n) {
            return Some(format!("{n}. {}", options[n - 1]));
        }
    }
    Some(t.to_string())
}

#[cfg(test)]
mod human_assist_tui_tests {
    use super::*;

    #[test]
    fn maps_empty_input_to_first_option() {
        let opts = vec!["已完成".to_string(), "取消".to_string()];
        assert_eq!(
            map_human_assist_input("", &opts),
            Some("1. 已完成".to_string())
        );
        assert_eq!(
            map_human_assist_input("  \n", &opts),
            Some("1. 已完成".into())
        );
    }

    #[test]
    fn maps_numeric_choice() {
        let opts = vec!["继续".to_string(), "跳过".to_string()];
        assert_eq!(map_human_assist_input("2", &opts), Some("2. 跳过".into()));
        assert_eq!(map_human_assist_input("99", &opts), Some("99".to_string()));
    }

    #[test]
    fn maps_cancel_keywords_to_none() {
        let opts = vec!["继续".to_string()];
        for kw in ["q", "Q", "取消", "cancel", "exit"] {
            assert_eq!(map_human_assist_input(kw, &opts), None, "kw={kw}");
        }
    }

    #[test]
    fn maps_free_text_verbatim() {
        let opts = vec!["已完成".to_string()];
        assert_eq!(
            map_human_assist_input("482913", &opts),
            Some("482913".to_string())
        );
    }
}

/// 判断编排层失败原因是否网络类(用于 D13 连接状态更新)。
fn looks_like_network_failure(reason: &str) -> bool {
    let lower = reason.to_lowercase();
    lower.contains("network")
        || lower.contains("connection")
        || lower.contains("timeout")
        || lower.contains("refused")
        || lower.contains("reset")
        || lower.contains("dns")
        || lower.contains("unreachable")
        || lower.contains("llm")
        || lower.contains("http")
}

/// 分类失败原因生成简短标签(用于 ConnectivityTracker 展示)。
fn categorize_failure_reason(reason: &str) -> String {
    let lower = reason.to_lowercase();
    if lower.contains("timeout") {
        "timeout"
    } else if lower.contains("refused") || lower.contains("connection") {
        "connection_refused"
    } else if lower.contains("dns") || lower.contains("resolve") {
        "dns_error"
    } else if lower.contains("reset") {
        "connection_reset"
    } else if lower.contains("http") || lower.contains("503") || lower.contains("502") {
        "http_error"
    } else {
        "network"
    }
    .to_string()
}
