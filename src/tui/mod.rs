//! TUI 交互界面 —— 独立 CLI 渲染引擎 + 斜杠命令 + 多轮对话。
//!
//! 架构见 `docs/TUI界面与CLI渲染引擎/02-技术设计.md`。
//! - 主屏:保留 0.1.2 的 `InputHandler` 单行输入 + 斜杠命令补全。
//! - 子屏:`engine.rs` 的 Screen 栈,接管 `/provider *` 系列。
//! - `/provider` 单独输入默认路由到 `/provider list`。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;

use crate::agent::debug::{finalize_report, DebugCollector, DebugLlmClient, ReportMeta};
use crate::agent::orchestrator::{MultiAgentOrchestrator, OrchestrationOutcome};
use crate::config::{Db, Paths, ProviderRecord};
use crate::llm::{client_from_record, ChatMessage};
use crate::session::Session;
use crate::tui::input::display_width;

pub mod branches;
pub mod commands;
pub mod completion;
pub mod engine;
pub mod export;
pub mod form;
pub mod mention;
pub mod pathfmt;
pub mod render;
pub mod screen;
pub mod theme;

mod input;

use branches::BranchStore;
use completion::CompletionEngine;
use export::{OutcomeKind, TranscriptEntry};
use input::{InputHandler, InputResult};

pub struct TuiSession {
    pub paths: Paths,
    pub db: Arc<Mutex<Db>>,
    pub orchestrator: MultiAgentOrchestrator,
    pub session: Session,
    /// 调试模式采集器(`-debug` 时 Some;每个用户任务前 reset)
    pub debug: Option<Arc<DebugCollector>>,
    /// 未装饰的原始 LLM 客户端(驱动 Debug Agent 评估,避免自我采集递归)
    pub debug_llm_raw: Option<Arc<dyn crate::llm::LlmClient>>,
    /// 会话导出 transcript(D8,2026-09-10):TUI 层独立记录,不动主 Session 上下文
    pub transcript: Vec<TranscriptEntry>,
    /// 会话累计 token 用量(每轮任务结束后累加,`/export` 元信息使用)
    pub session_usage: crate::llm::Usage,
    /// 对话分支存储(D3,2026-09-10 第二十四轮):rewind/fork/switch/clear 前自动快照
    pub branches: BranchStore,
    /// 当前任务开始时间戳(2026-09-10 第 27 轮 F05):handle_normal_prompt 入口
    /// 设置 Some,print_usage 后清回 None;Some 期间 print_usage 追加「(耗时 Ns)」。
    pub task_started_at: Option<std::time::Instant>,
}

impl TuiSession {
    pub fn bootstrap() -> Result<Self> {
        Self::bootstrap_with_debug(false)
    }

    /// 启动 TUI 会话;`debug=true` 时开启调试采集(对应 `laew -debug`)。
    pub fn bootstrap_with_debug(debug: bool) -> Result<Self> {
        let paths = Paths::detect().map_err(anyhow::Error::from)?;
        let db = Db::open(&paths).map_err(anyhow::Error::from)?;
        let db = Arc::new(Mutex::new(db));
        let plans_dir = paths.root_dir.join("plans");
        let session = Session::new();
        let collector = debug.then(|| Arc::new(DebugCollector::new(session.id())));
        let (orchestrator, debug_llm_raw) =
            build_orchestrator_with_active(&db, plans_dir, collector.clone())?;
        Ok(Self {
            paths,
            db,
            orchestrator,
            session,
            debug: collector,
            debug_llm_raw,
            transcript: Vec::new(),
            session_usage: crate::llm::Usage::default(),
            branches: BranchStore::new(),
            task_started_at: None,
        })
    }

    /// 按当前 active provider 重建 orchestrator(debug 模式同时刷新原始 LLM 引用)。
    fn rebuild_orchestrator(&mut self) -> Result<()> {
        let plans_dir = self.paths.root_dir.join("plans");
        let (orch, raw) = build_orchestrator_with_active(&self.db, plans_dir, self.debug.clone())?;
        self.orchestrator = orch;
        self.debug_llm_raw = raw;
        Ok(())
    }

    pub fn print_banner(&self) {
        let active = self.db.lock().expect("db").get_active_or_env().ok().flatten();
        println!("╔══════════════════════════════════════════════════════════╗");
        println!(
            "║  LsmAgentEmergentWork  ·  laew  TUI  ·  v{}           ║",
            env!("CARGO_PKG_VERSION")
        );
        println!(
            "║  编译时间: {}                          ║",
            env!("LAEW_BUILD_TIME")
        );
        println!("╠══════════════════════════════════════════════════════════╣");
        println!(
            "║  根目录 : {} ║",
            fit_display(&self.paths.root_dir.display().to_string(), 46)
        );
        println!(
            "║  工作目录: {} ║",
            fit_display(&self.paths.work_dir.display().to_string(), 45)
        );
        // 项目说明文件状态(纯探测,不触发生成;发现规则见 docs/Yolo项目上下文注入/)
        let doc_source = crate::agent::project_context::probe(&self.paths.work_dir);
        println!("║  项目说明: {} ║", fit_display(doc_source.as_str(), 45));
        match active {
            Some(r) => println!(
                "║  当前模型: {} ║",
                fit_display(
                    &format!(
                        "[{}] {}/{} @ {}",
                        r.protocol.as_str(),
                        r.provider_name,
                        r.model_name,
                        r.end_point
                    ),
                    45,
                )
            ),
            None => println!(
                "║  当前模型: {} ║",
                fit_display("<未配置, 使用 /provider add 添加>", 45)
            ),
        }
        println!("║  Session: {} ║", fit_display(&self.session.id, 46));
        // D12 主题提示行(2026-09-10 第二十三轮):告知用户当前主题与切换方式
        let active_theme = crate::tui::theme::active_kind();
        println!(
            "║  主  题 : {} ║",
            fit_display(&format!("{}  (切换 /theme [kind])", active_theme.as_str()), 46)
        );
        println!("╚══════════════════════════════════════════════════════════╝");
        println!("  输入提示词开始对话, 输入 / 查看可用命令。");
        println!("  快捷键: ↑↓ 选择补全  Enter 提交  Esc 关闭补全  Ctrl-D 退出");
        println!();
    }

    /// 切换当前 provider(根据 id),并重新构造 MultiAgentOrchestrator
    pub fn switch_provider(&mut self, id: i64) -> Result<()> {
        let record = self
            .db
            .lock()
            .expect("db")
            .get(id)
            .map_err(anyhow::Error::from)?;
        self.db
            .lock()
            .expect("db")
            .set_active(id)
            .map_err(anyhow::Error::from)?;
        let _ = record;
        self.rebuild_orchestrator()?;
        Ok(())
    }

    /// 重置会话:清空上下文并生成新 Session ID。
    /// transcript 与累计用量随会话一并清零(导出的是「当前会话」)。
    ///
    /// D3(2026-09-10 第二十四轮):清空前若有真实对话轮次,自动快照为
    /// `clear-k` 分支(误触 /clear 可 `/switch` 找回),返回分支名。
    pub fn reset_session(&mut self) -> Option<String> {
        let saved = self.snapshot_current("clear", "清空前");
        self.session = Session::new();
        self.transcript.clear();
        self.session_usage = crate::llm::Usage::default();
        saved
    }

    /// 把当前会话状态快照为分支,返回分支名(无真实轮次时返回 None)。
    fn snapshot_current(&mut self, prefix: &str, note_prefix: &str) -> Option<String> {
        let turns = crate::agent::session_fork::scan_user_turns(self.session.context());
        if turns.is_empty() {
            return None;
        }
        let note = format!("{note_prefix}(共 {} 轮)", turns.len());
        Some(self.branches.snapshot(
            prefix,
            &note,
            &self.session,
            &self.transcript,
            self.session_usage,
            export::now_clock(),
        ))
    }

    pub fn add_provider_interactive(&self) -> Result<i64> {
        println!("  新增接入记录:");
        let protocol = read_line_prompt("    protocol (anthropic/openai): ")?;
        let protocol =
            crate::config::Protocol::parse(protocol.trim()).map_err(anyhow::Error::from)?;
        let provider_name = read_line_prompt("    provider_name: ")?;
        let model_name = read_line_prompt("    model_name: ")?;
        let end_point = read_line_prompt("    end_point: ")?;
        let api_key = read_line_prompt("    api_key: ")?;
        let id = self
            .db
            .lock()
            .expect("db")
            .add(
                protocol,
                provider_name.trim(),
                model_name.trim(),
                end_point.trim(),
                api_key.trim(),
            )
            .map_err(anyhow::Error::from)?;
        println!("  ✓ 已新增接入记录 id={id}");
        Ok(id)
    }

    pub fn list_providers(&self) -> Result<()> {
        let records = self
            .db
            .lock()
            .expect("db")
            .list()
            .map_err(anyhow::Error::from)?;
        if records.is_empty() {
            println!("  (空)尚未配置任何接入记录。");
            return Ok(());
        }
        for r in &records {
            print_record(r);
        }
        Ok(())
    }

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
    async fn dispatch_prompt(&mut self, raw: &str, prompt: &str) -> Result<bool> {
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
                ..
            }) => {
                println!();
                println!("  [agent failed]");
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
    fn print_diff_hunk(&self, hunk: &crate::tui::render::diff::DiffHunk) {
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

    /// 处理斜杠命令。`cmd` 为去掉 `/` 前缀的命令串;`full_line` 为用户原始输入行
    /// (自定义命令 dispatch 时作为 transcript 的 raw_input,保留 `/cmd args` 原文)。
    async fn handle_slash(&mut self, cmd: &str, full_line: &str) -> Result<bool> {
        let mut it = cmd.split_whitespace();
        let head = it.next().unwrap_or("");
        // 头部之后的剩余参数(保留原始空白语义,自定义命令 /export 路径均使用)
        let rest_args = cmd[head.len()..].trim();
        match head {
            "help" | "h" | "?" => {
                print_help();
            }
            "exit" | "quit" | "q" => {
                println!("  再见。");
                return Ok(true);
            }
            "clear" | "c" => {
                let saved = self.reset_session();
                // TUI 主屏下真正清屏:滚动区清空(ANSI 100 行上滚) + 重新打印 banner。
                // 修复前只 print 一行 Session ID,旧对话历史与提示词残留在视觉上不被清除,
                // 用户体感「清屏没生效」。
                if atty() {
                    print!("\x1b[2J\x1b[H");
                    self.print_banner();
                } else {
                    println!(
                        "  已清空对话历史并开启新会话, Session ID: {}",
                        self.session.id
                    );
                }
                if let Some(name) = saved {
                    println!("  (已自动保存分支 {name},可用 /switch {name} 找回)");
                }
            }
            "new" | "n" => {
                let saved = self.reset_session();
                if atty() {
                    print!("\x1b[2J\x1b[H");
                    self.print_banner();
                } else {
                    println!("  已开启新会话, Session ID: {}", self.session.id);
                }
                if let Some(name) = saved {
                    println!("  (已自动保存分支 {name},可用 /switch {name} 找回)");
                }
            }
            "model" => {
                if let Some(r) = self
                    .db
                    .lock()
                    .expect("db")
                    .get_active_or_env()
                    .map_err(anyhow::Error::from)?
                {
                    println!(
                        "  [{}] {} / {}  @ {}",
                        r.protocol.as_str(),
                        r.provider_name,
                        r.model_name,
                        r.end_point
                    );
                } else {
                    println!("  当前未配置模型。");
                }
            }
            "provider" | "p" => {
                let sub = it.next().unwrap_or("");
                match sub {
                    "list" | "ls" => {
                        // 进入 ProviderList 屏
                        self.run_provider_list_screen().await?;
                    }
                    "add" => {
                        // 进入 ProviderForm 屏(add 模式)
                        self.run_provider_add_screen().await?;
                    }
                    "use" => {
                        let id_str = it.next().unwrap_or("");
                        if let Ok(id) = id_str.parse::<i64>() {
                            self.switch_provider(id)?;
                            println!("  ✓ 已切换到 id={id}");
                        } else {
                            println!("  用法: /provider use <id>");
                        }
                    }
                    "del" | "delete" | "rm" => {
                        // 进入 ProviderDelPicker 屏
                        self.run_provider_del_screen().await?;
                    }
                    "" => {
                        // 单独 /provider 默认路由到 list
                        self.run_provider_list_screen().await?;
                    }
                    other => println!("  未知 /provider 子命令: {other}"),
                }
            }
            // D8 会话导出:默认落工作目录 Markdown,显式路径按后缀定格式
            "export" => {
                self.run_export(rest_args);
            }
            // D2 自定义命令自观测:列出已加载命令与来源
            "commands" => {
                self.print_custom_commands();
            }
            "diff" => {
                // /diff <old_file> <new_file>:并排 diff 两个文件(行级+字符级着色)
                let parts: Vec<&str> = rest_args.split_whitespace().collect();
                if parts.len() < 2 {
                    println!("  用法: /diff <旧文件路径> <新文件路径>");
                    println!("  示例: /diff a.rs b.rs");
                } else {
                    let old_path = parts[0];
                    let new_path = parts[1];
                    match crate::tui::render::diff::diff_files(old_path, new_path) {
                        Ok(hunk) => {
                            self.print_diff_hunk(&hunk);
                        }
                        Err(e) => {
                            println!("  [diff 错误] 无法读取文件: {e}");
                        }
                    }
                }
            }
            "theme" | "t" => {
                // D12 多主题切换命令(2026-09-10 第二十三轮)
                self.run_theme(rest_args);
            }
            // D3 对话 Rewind / 分支(2026-09-10 第二十四轮)
            "rewind" => {
                self.run_rewind(rest_args);
            }
            "undo" => {
                self.run_undo();
            }
            "fork" => {
                self.run_fork();
            }
            "branches" | "branch" => {
                self.run_branches();
            }
            "switch" => {
                self.run_switch(rest_args);
            }
            "" => {}
            other => {
                // 内置未命中 → 查自定义命令(D2);命中则渲染模板并送编排。
                // 内置命令不可遮蔽:与内置同名时根本走不进此分支。
                let customs = commands::discover(&self.paths.work_dir);
                match commands::render_by_name(&customs, other, rest_args) {
                    Some(rendered) => {
                        let source = customs
                            .iter()
                            .find(|c| c.name == other)
                            .map(|c| c.source.display().to_string())
                            .unwrap_or_default();
                        println!("  [custom command] /{other} ← {source}");
                        return self.dispatch_prompt(full_line, &rendered).await;
                    }
                    None => {
                        println!("  未知斜杠命令: /{other}");
                        let suggestions = suggest_similar_commands(other);
                        if !suggestions.is_empty() {
                            println!("  您是否想输入: {}", suggestions.join(", "));
                        }
                        println!("  输入 /help 查看所有命令, /commands 查看自定义命令。");
                    }
                }
            }
        }
        Ok(false)
    }

    /// `/theme [kind]`(D12,2026-09-10 第二十三轮):列出或切换主题。
    ///
    /// - 无参数:列出全部主题 + 当前活跃主题 + 实时生效范围说明
    /// - 有参数:解析 → 切换;返回旧主题供输出「✓ 已从 X 切换到 Y」
    ///
    /// 实时生效范围(本轮实现):
    ///   - 主屏 banner 主题行:下次会话或 `/clear` 重打 banner
    ///   - 边框 (engine::border_box):下次子屏重绘
    ///   - diff 标题/行号/前后缀/字符级着色:下次 `/diff` 调用
    ///   - 语法高亮 token:下次围栏渲染
    ///   - Cell::blank() 空白底:全屏实时
    /// 不实时(需重启):子屏硬编码 const(SELECTED_FG / INPUT_BG 等),所以打印友好提示
    fn run_theme(&self, arg: &str) {
        let current = crate::tui::theme::active_kind();
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            println!("  当前主题: {}", current.as_str());
            println!("  可用主题:");
            for k in crate::tui::theme::all_kinds() {
                let marker = if *k == current { " * " } else { "   " };
                println!("    {marker}{:<14} {}", k.as_str(), k.describe());
            }
            println!("  切换主题:");
            println!("    /theme <kind>   例如: /theme dark-contrast");
            println!("    LAEW_THEME=dark-contrast ./laew   (启动期初始化)");
            println!("  注:核心 cell 渲染(banner / 边框 / diff / 高亮)实时生效;");
            println!("      子屏局部配色需重启会话才能完整生效。");
            return;
        }
        match crate::tui::theme::ThemeKind::from_env_str(trimmed) {
            Some(kind) => {
                let prev = crate::tui::theme::set_active(kind);
                println!("  ✓ 已切换主题: {} → {}", prev.as_str(), kind.as_str());
                println!("    核心 cell 渲染实时生效(下次重绘可见)。");
                println!("    子屏局部配色需重启会话才能完整生效。");
            }
            None => {
                println!("  未知主题: {trimmed}");
                println!("  可选: default | dark-contrast | light | daltonized");
                println!("  输入 /theme 查看主题列表与说明。");
            }
        }
    }

    /// `/rewind [N]`(D3,2026-09-10 第二十四轮):列出轮次或回退到第 N 轮之前。
    ///
    /// - 无参数:列出当前会话全部真实轮次(#编号 + 时间 + 首行预览)
    /// - `/rewind N`:第 N..末轮全部移除(回退前自动快照存分支),
    ///   context / transcript / session_usage 三处一致截断
    fn run_rewind(&mut self, arg: &str) {
        let turns = crate::agent::session_fork::scan_user_turns(self.session.context());
        if turns.is_empty() {
            println!("  当前会话还没有可回退的对话轮次。");
            return;
        }
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            println!("  可回退的对话轮次(共 {} 轮,从旧到新):", turns.len());
            for t in &turns {
                let ts = self
                    .transcript
                    .get(t.order - 1)
                    .map(|e| e.ts.as_str())
                    .unwrap_or("-");
                let preview = first_line_preview(&t.prompt, 48);
                println!("    #{} [{}] {}", t.order, ts, preview);
            }
            println!("  用法: /rewind <编号>  回退到该轮之前(该轮及其后全部移除,原对话自动存为分支)");
            println!("        /undo           撤销最后一轮(等价 /rewind {})", turns.len());
            return;
        }
        match trimmed.parse::<usize>() {
            Ok(order) => self.run_rewind_order(order),
            Err(_) => {
                println!("  无效轮次编号: {trimmed}(应为 1..={} 的整数)", turns.len());
                println!("  输入 /rewind 查看全部轮次。");
            }
        }
    }

    /// 执行回退到第 `order` 轮之前(编号已由调用方给出,此处负责校验与三处一致截断)。
    fn run_rewind_order(&mut self, order: usize) {
        let turns = crate::agent::session_fork::scan_user_turns(self.session.context());
        if order == 0 || order > turns.len() {
            println!(
                "  无效轮次编号: {order}(当前共 {} 轮,编号 1..={})",
                turns.len(),
                turns.len()
            );
            println!("  输入 /rewind 查看全部轮次。");
            return;
        }
        let boundary = turns[order - 1].ctx_index;
        let removed_turns = turns.len() - order + 1;
        let removed_msgs = self.session.context().len() - boundary;
        // 破坏性操作前快照(atomcode RewindTransactionGuard 语义:截断与快照同生成败)
        let name = self
            .snapshot_current("rewind", &format!("/rewind {order} 回退前"))
            .expect("调用方已确认存在真实轮次");
        self.session.context_mut().truncate(boundary);
        self.transcript.truncate(order - 1);
        // 累计用量由剩余轮次重算(被移除轮次的 token 不再计入「当前会话」)
        self.session_usage = self
            .transcript
            .iter()
            .fold(crate::llm::Usage::default(), |acc, e| merge_usage(acc, e.usage));
        println!(
            "  ✓ 已回退到第 {order} 轮之前(移除 {removed_turns} 轮对话 / {removed_msgs} 条上下文消息)"
        );
        if order == 1 {
            println!("    已清空全部真实轮次(项目上下文等内部标记保留,不会重复注入)。");
        } else {
            println!("    保留第 1..={} 轮,当前上下文 {} 条消息。", order - 1, boundary);
        }
        println!("    原对话已存为分支 {name},可用 /switch {name} 找回。");
    }

    /// `/undo`(D3):撤销最后一轮(最常用路径一键化,等价 `/rewind <末轮>`)。
    fn run_undo(&mut self) {
        let n = crate::agent::session_fork::scan_user_turns(self.session.context())
            .len();
        if n == 0 {
            println!("  当前会话还没有可回退的对话轮次。");
            return;
        }
        self.run_rewind_order(n);
    }

    /// `/fork`(D3,pi `/clone` current leaf 语义):从当前对话分叉出新 Session。
    ///
    /// 上下文完整拷贝 + 新 Session ID,后续对话在新会话上进行;
    /// 原对话自动存分支(两个方向都不会丢)。
    fn run_fork(&mut self) {
        let turns = crate::agent::session_fork::scan_user_turns(self.session.context());
        if turns.is_empty() {
            println!("  当前会话没有对话轮次,无需分叉(直接输入提示词即可)。");
            return;
        }
        let name = self
            .snapshot_current("fork", "fork 前原会话")
            .expect("调用方已确认存在真实轮次");
        let forked = Session::fork_from(&self.session);
        let new_id = forked.id.clone();
        let msg_count = forked.context.len();
        self.session = forked;
        println!("  ✓ 已从当前对话分叉出新会话: {new_id}");
        println!("    上下文 {msg_count} 条消息 / {} 轮完整保留,后续对话在新会话上进行。", turns.len());
        println!("    原对话已存为分支 {name},可用 /switch {name} 找回。");
    }

    /// `/branches`(D3):列出已存分支(新→旧)。
    fn run_branches(&self) {
        if self.branches.is_empty() {
            println!("  暂无分支。/rewind <n>、/fork、/switch、/clear 在改动前会自动保存分支。");
            return;
        }
        println!("  已存分支({} 个,新→旧):", self.branches.len());
        for s in self.branches.list() {
            let preview = if BranchStore::last_turn_preview(s).is_empty() {
                "-".to_string()
            } else {
                first_line_preview(&BranchStore::last_turn_preview(s), 32)
            };
            println!(
                "    {} [{}] {} 轮 | {} | 末轮: {}",
                s.name,
                s.created_at,
                s.transcript.len(),
                s.note,
                preview
            );
        }
        println!("  切换: /switch <分支名>;分支保存在内存中(最多 10 个,超出淘汰最旧),退出 TUI 后失效。");
    }

    /// `/switch <name>`(D3):切换到指定分支;切换前当前对话自动快照(零丢失)。
    fn run_switch(&mut self, arg: &str) {
        let name = arg.trim();
        if name.is_empty() {
            println!("  用法: /switch <分支名>(输入 /branches 查看可用分支)");
            return;
        }
        if self.branches.get(name).is_none() {
            println!("  未找到分支: {name}(输入 /branches 查看可用分支)");
            return;
        }
        let cur = self.snapshot_current("switch", &format!("/switch {name} 切换前"));
        let (session, transcript, usage) = self
            .branches
            .restore(name)
            .expect("上面已校验分支存在");
        let restored_id = session.id.clone();
        let restored_turns = transcript.len();
        self.session = session;
        self.transcript = transcript;
        self.session_usage = usage;
        println!("  ✓ 已切换到分支 {name}(Session {restored_id}, {restored_turns} 轮对话)");
        if let Some(cur) = cur {
            println!("    切换前的对话已自动存为分支 {cur},可再切回。");
        } else {
            println!("    切换前会话无真实轮次,未产生快照。");
        }
    }

    /// `/export [path]`(D8):导出当前会话 transcript。
    fn run_export(&mut self, path_arg: &str) {
        let model = match self.db.lock().expect("db").get_active_or_env() {
            Ok(Some(r)) => format!("[{}] {}/{}", r.protocol.as_str(), r.provider_name, r.model_name),
            _ => "<未配置>".to_string(),
        };
        let default_ts = export::now_export_stamp(); // YYYYMMDD-HHMMSS
        let explicit = if path_arg.trim().is_empty() {
            None
        } else {
            Some(path_arg.trim())
        };
        let meta = export::ExportMeta {
            session_id: self.session.id.clone(),
            session_created_at: export::humanize_compact(&self.session.created_at),
            exported_at: export::now_export_human(),
            model,
            turns: self.transcript.len(),
            total_usage: self.session_usage,
        };
        match export::resolve_target(&self.paths.work_dir, explicit, "laew-export", &default_ts) {
            Ok((path, fmt)) => {
                let fmt_name = if fmt == export::ExportFormat::Json { "JSON" } else { "Markdown" };
                match export::write_export(&meta, &self.transcript, &path, fmt) {
                    Ok(()) => println!(
                        "{}",
                        pathfmt::fit_line(
                            &format!("  ✓ 已导出 {fmt_name}: "),
                            &pathfmt::display_path(&self.paths, &path),
                            &format!(" ({} 轮对话)", meta.turns)
                        )
                    ),
                    Err(e) => eprintln!("  导出失败: {e}"),
                }
            }
            Err(e) => eprintln!("  导出失败: {e}"),
        }
    }

    /// `/commands`(D2):列出已加载的自定义命令(名称/描述/来源)。
    fn print_custom_commands(&self) {
        let customs = commands::discover(&self.paths.work_dir);
        if customs.is_empty() {
            println!("  当前无自定义命令。创建方法(Markdown 模板):");
            println!(
                "    项目级: {}/.laew/commands/<命令名>.md",
                self.paths.work_dir.display()
            );
            println!("    用户级: ~/.laew/commands/<命令名>.md");
            println!("    模板内可用 $ARGUMENTS(全量参数)与 $1-$9(位置参数)");
            // 静默失败可诊断化(第 23 轮):列出「存在 .md 但文件名非法被跳过」的项
            let ignored = commands::scan_invalid_names(&self.paths.work_dir);
            if !ignored.is_empty() {
                println!("  ⚠ 发现未加载的命令文件(文件名不合法):");
                for line in ignored {
                    println!("    {line}");
                }
            }
            return;
        }
        println!("  自定义命令({} 个,用户级优先于项目级):", customs.len());
        for c in &customs {
            let hint = if c.argument_hint.is_empty() {
                String::new()
            } else {
                format!(" {}", c.argument_hint)
            };
            println!("  /{}{hint}", c.name);
            println!("    {} — {}", c.description, c.source.display());
        }
    }

    /// 进入 ProviderList 屏(子屏通过 engine 渲染)。
    /// 当 stdin 不是 TTY(如 e2e 管道)时,回退到 print 输出以保持兼容。
    async fn run_provider_list_screen(&mut self) -> Result<()> {
        use crate::tui::engine::{enter_alt, leave_alt};
        use crate::tui::screen::provider_list::ProviderList;

        if !atty() {
            // 非 TTY:回退到 print 输出
            return self.list_providers();
        }

        let screen: Box<dyn crate::tui::engine::Screen> =
            Box::new(ProviderList::new(self.db.clone(), self.paths.clone()));
        enter_alt().map_err(anyhow::Error::from)?;
        let toast = Self::run_screen_loop(screen).await?;
        leave_alt().map_err(anyhow::Error::from)?;
        // Toast 必须在 leave_alt 之后输出,否则会被 alternate screen 切换吞掉
        // (第三十轮 Bug 修复)
        if let Some(msg) = toast {
            println!("  {msg}");
        }
        // 重建 YoloRunner(可能切换了 use)
        self.rebuild_orchestrator()?;
        Ok(())
    }

    async fn run_provider_add_screen(&mut self) -> Result<()> {
        use crate::tui::engine::{enter_alt, leave_alt};
        use crate::tui::screen::provider_form::ProviderForm;

        if !atty() {
            return self.add_provider_interactive().map(|_| ());
        }

        let db = self.db.clone();
        let on_done = Box::new(move |id: i64| {
            let _ = db.lock().expect("db").set_active(id);
        });
        let screen: Box<dyn crate::tui::engine::Screen> = Box::new(ProviderForm::new_add(
            self.db.clone(),
            self.paths.clone(),
            on_done,
        ));
        enter_alt().map_err(anyhow::Error::from)?;
        let toast = Self::run_screen_loop(screen).await?;
        leave_alt().map_err(anyhow::Error::from)?;
        // Toast 必须在 leave_alt 之后输出,否则会被 alternate screen 切换吞掉
        if let Some(msg) = toast {
            println!("  {msg}");
        }
        self.rebuild_orchestrator()?;
        Ok(())
    }

    async fn run_provider_del_screen(&mut self) -> Result<()> {
        use crate::tui::engine::{enter_alt, leave_alt};
        use crate::tui::screen::provider_del::ProviderDelPicker;

        if !atty() {
            // 非 TTY:提示使用 CLI 子命令
            println!("  非交互模式请使用: laew provider del <id>");
            return Ok(());
        }

        let screen: Box<dyn crate::tui::engine::Screen> = Box::new(ProviderDelPicker::new(
            self.db.clone(),
            self.paths.clone(),
            -1,
        ));
        enter_alt().map_err(anyhow::Error::from)?;
        let toast = Self::run_screen_loop(screen).await?;
        leave_alt().map_err(anyhow::Error::from)?;
        // Toast 必须在 leave_alt 之后输出,否则会被 alternate screen 切换吞掉
        if let Some(msg) = toast {
            println!("  {msg}");
        }
        self.rebuild_orchestrator()?;
        Ok(())
    }

    /// 通用子屏循环:渲染 → 读键 → 处理 Outcome。
    /// 屏幕栈:`Vec<Box<dyn Screen>>`,top 是当前屏;`Push` 压栈、`Pop` 出栈。
    /// 当栈清空时退出循环(回到主屏)。
    ///
    /// 返回值:`Result<Option<String>>` —— `Some(msg)` 表示子屏触发了
    /// `Outcome::Toast(msg)`,由调用者在退出 alternate screen 后输出到主屏。
    /// 第三十轮 Bug 修复:之前在子屏内直接 `println!` 会被 alternate screen
    /// 切换吞掉,用户看不到任何反馈。
    async fn run_screen_loop(
        initial: Box<dyn crate::tui::engine::Screen>,
    ) -> Result<Option<String>> {
        use crate::tui::engine::{present, read_key, Frame, Outcome, Rect};

        let mut stack: Vec<Box<dyn crate::tui::engine::Screen>> = Vec::new();
        stack.push(initial);

        // 栈中每层屏都进入一次
        for s in stack.iter_mut() {
            s.on_enter();
        }

        while let Some(top) = stack.last_mut() {
            let area = Rect::full_screen();
            let mut frame = Frame::new(area);
            top.render(&mut frame);
            present(&frame).map_err(anyhow::Error::from)?;

            let key = read_key().map_err(anyhow::Error::from)?;
            // 用 take() 取出 Outcome 后再处理,避免借用冲突
            let outcome = top.handle_key(key);
            match outcome {
                Outcome::Continue => {}
                Outcome::Pop => {
                    let mut popped = stack.pop().unwrap();
                    popped.on_exit();
                }
                Outcome::Push(mut new_screen) => {
                    new_screen.on_enter();
                    stack.push(new_screen);
                }
                Outcome::Toast(msg) => {
                    // Toast:弹出现有屏栈,把消息回给调用者,在主屏上下文打印。
                    // 修复点:不在 alt screen 内 println,避免被 leave_alt 吞掉。
                    while let Some(mut s) = stack.pop() {
                        s.on_exit();
                    }
                    return Ok(Some(msg));
                }
                Outcome::Quit => std::process::exit(0),
            }
        }
        Ok(None)
    }
}

/// 格式化任务执行结果为多行文本(D8:屏幕打印与导出同源,避免两处漂移)。
fn format_task_result(
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
            cache.push_str(&format!("  cache_creation={}", usage.cache_creation_input_tokens));
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
fn format_task_result_for_context(
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
            crate::agent::quality::Verdict::Pass => {
                ("(质检通过)".to_string(), String::new())
            }
            crate::agent::quality::Verdict::Fail => {
                let mut issues_str = String::new();
                if !wf.quality_report.issues.is_empty() {
                    issues_str = format!(
                        ";问题:{}",
                        wf.quality_report.issues.join("; ")
                    );
                }
                ("(质检未通过)".to_string(), issues_str)
            }
        };
        out.push_str(&format!("{verdict_text}{issues_text}\n"));
    }
    out
}
/// Usage 逐字段饱和累加(会话累计用途;orchestrator 内部 add_usage 为私有,此处独立实现)。
fn merge_usage(a: crate::llm::Usage, b: crate::llm::Usage) -> crate::llm::Usage {
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
fn now_clock() -> String {
    export::now_clock()
}

/// waiting 心跳行文案(纯函数,便于单测;2026-09-10 第 28 轮 B09/B10 抽取):
/// - `Some(stage)`:阶段内等待,`  [waiting] {stage}  {frame}  ({elapsed}s){slow_warn}`
/// - `None`:初始 spinner(首阶段消息未到),`  [waiting] {frame}  ({elapsed}s){slow_warn}`
/// - elapsed ≥ 60s 追加「等待超过 1 分钟」提示;≥ 30s 追加「响应较慢」。
/// 初始 spinner 同样启用 30s/60s 慢提示 —— 修复前固定 `(0s)`,真实慢 LLM
/// (首字节 >30s)场景下用户既看不到计时也看不到慢提示。
fn waiting_line_text(stage: Option<&str>, frame: char, elapsed_secs: u64) -> String {
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
fn truncate_chars(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(limit.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// 建议相似命令（简单的编辑距离近似）。
fn suggest_similar_commands(input: &str) -> Vec<String> {
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
fn fit_display(s: &str, max: usize) -> String {
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
            if w + cw > max_len as u16 { break; }
            out.push(c);
            w += cw;
        }
        out + "…"
    }
}

/// 取首行非空内容并压缩空白,截断到 `max` 显示宽度(D3 轮次/分支列表预览用)。
fn first_line_preview(s: &str, max: usize) -> String {
    let first = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let collapsed: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&collapsed, max)
}

/// 简单的标准输入读取（用于交互子命令）。
fn read_line_prompt(prompt: &str) -> Result<String> {
    use std::io::{self, BufRead};
    print!("{prompt}");
    io::Write::flush(&mut io::stdout())?;
    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn print_record(r: &ProviderRecord) {
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

fn print_help() {
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

/// 把 `theme::attr` 位掩码转换为 ANSI 转义前缀(Bold/Underlined/DIM)。
fn attrs_to_ansi(attrs: u8) -> String {
    let mut s = String::new();
    if attrs & crate::tui::theme::attr::BOLD != 0 {
        s.push_str("\x1b[1m");
    }
    if attrs & crate::tui::theme::attr::DIM != 0 {
        s.push_str("\x1b[2m");
    }
    if attrs & crate::tui::theme::attr::UNDERLINED != 0 {
        s.push_str("\x1b[4m");
    }
    if attrs & crate::tui::theme::attr::REVERSE != 0 {
        s.push_str("\x1b[7m");
    }
    s
}

/// 把背景色转为 ANSI 背景序列(2026-09-10 第二十五轮 F04/B07 测试新增)。
/// `Color::Reset` 返回空串(不输出 bg 序列,沿用终端默认底色)。
/// 非 Reset 时返回 `\x1b[48;5;{idx}m` 形式,256 色背景块。
fn bg_color_ansi(bg: crossterm::style::Color) -> String {
    use crossterm::style::Color;
    match bg {
        Color::Reset => String::new(),
        _ => format!("\x1b[48;5;{}m", color_to_ansi256(bg)),
    }
}

#[cfg(test)]
mod bg_color_ansi_tests {
    use super::*;
    use crossterm::style::Color;

    #[test]
    fn reset_returns_empty_string() {
        // Color::Reset 不输出 bg 序列(避免污染终端默认底色)。
        assert_eq!(bg_color_ansi(Color::Reset), "");
    }

    #[test]
    fn non_reset_emits_bg48_5_idx_m() {
        // 非 Reset 输出 `\x1b[48;5;{idx}m`,与 fg 输出的 `\x1b[38;5;{idx}m` 对称。
        let s = bg_color_ansi(Color::DarkGreen);
        assert!(s.starts_with("\x1b[48;5;"), "got: {s}");
        assert!(s.ends_with("m"), "got: {s}");
        // DarkGreen 在 color_to_ansi256 里映射到索引 2。
        assert_eq!(s, "\x1b[48;5;2m");
    }
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
        assert!(s.starts_with("  [waiting] wf-1 SubAgent 执行中…  ⠧  (8s)"), "got: {s}");
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
/// 把 crossterm Color 转为 ANSI 256 色索引(简化映射)。
fn color_to_ansi256(color: crossterm::style::Color) -> u8 {
    use crossterm::style::Color;
    match color {
        Color::Reset => 7,        // 白色/默认
        Color::Black => 0,
        Color::DarkRed => 1,
        Color::DarkGreen => 2,
        Color::DarkYellow => 3,
        Color::DarkBlue => 4,
        Color::DarkMagenta => 5,
        Color::DarkCyan => 6,
        Color::DarkGrey => 8,
        Color::Grey => 7,
        Color::Red => 9,
        Color::Green => 10,
        Color::Yellow => 11,
        Color::Blue => 12,
        Color::Magenta => 13,
        Color::Cyan => 14,
        Color::White => 15,
        Color::Rgb { r, g, b } => {
            // 简化 RGB → 256 色(取 6x6x6 立方体索引)
            let r_idx = if r < 48 { 0 } else { (r - 35) / 40 };
            let g_idx = if g < 48 { 0 } else { (g - 35) / 40 };
            let b_idx = if b < 48 { 0 } else { (b - 35) / 40 };
            let r_idx = r_idx.min(5) as u8;
            let g_idx = g_idx.min(5) as u8;
            let b_idx = b_idx.min(5) as u8;
            16 + 36 * r_idx + 6 * g_idx + b_idx
        }
        Color::AnsiValue(v) => v,
    }
}

/// 启动 MultiAgentOrchestrator(6 角色);若未配置,使用占位提示信息。
/// `debug` 为 Some 时:LLM 客户端包 DebugLlmClient 装饰器并注入采集器,
/// 同时返回未装饰的原始客户端(供 Debug Agent 评估使用)。
fn build_orchestrator_with_active(
    db: &Arc<Mutex<Db>>,
    plans_dir: PathBuf,
    debug: Option<Arc<DebugCollector>>,
) -> Result<(
    MultiAgentOrchestrator,
    Option<Arc<dyn crate::llm::LlmClient>>,
)> {
    // 构造期 User-Agent 仅作兜底默认值:Agent 循环按当前 profile 逐请求覆盖
    // (RequestMeta::user_agent,第 08 轮),8 角色在抓包层面各自可辨识
    let work_profile = crate::agent::profile::AgentProfile::work_profile();
    let user_agent = work_profile.user_agent();
    // 把 db 从 Mutex 拷一份裸出来(本次只读使用);Db 内部已有自己的 Mutex
    let db_clone = db.lock().expect("db").clone();
    let db_arc = Arc::new(db_clone);
    let raw_llm: Arc<dyn crate::llm::LlmClient> = match db
        .lock()
        .expect("db")
        .get_active_or_env()
        .map_err(anyhow::Error::from)?
    {
        Some(r) => client_from_record(&r, &user_agent).map_err(anyhow::Error::from)?,
        // 未配置时,6 个 Agent 都用 NoopLlm
        None => Arc::new(NoopLlm),
    };
    match debug {
        Some(collector) => {
            let decorated: Arc<dyn crate::llm::LlmClient> =
                Arc::new(DebugLlmClient::new(raw_llm.clone(), collector.clone()));
            let cfg = crate::agent::orchestrator::OrchestratorConfig {
                debug: Some(collector),
                ..Default::default()
            };
            Ok((
                MultiAgentOrchestrator::with_config(decorated, db_arc, plans_dir, cfg),
                Some(raw_llm),
            ))
        }
        None => Ok((
            MultiAgentOrchestrator::new(raw_llm, db_arc, plans_dir),
            None,
        )),
    }
}

/// 未配置模型时的占位 LLM(避免 null deref);`complete` 返回错误提示。
struct NoopLlm;

#[async_trait::async_trait]
impl crate::llm::LlmClient for NoopLlm {
    async fn complete(
        &self,
        _system: &str,
        _messages: &[ChatMessage],
        _tools: &[crate::llm::ToolDef],
        _meta: &crate::llm::RequestMeta,
    ) -> crate::error::Result<crate::llm::Completion> {
        Ok(crate::llm::Completion {
            text: "尚未配置大模型接入记录, 请先使用 `laew provider add` 或 TUI 内 `/provider add` 完成配置。".to_string(),
            tool_calls: vec![],
            usage: Default::default(),
            stop_reason: None,
        })
    }

    /// 占位客户端默认返回 Anthropic(仅影响系统提示词渲染,不会实际发起请求)。
    fn protocol(&self) -> crate::config::Protocol {
        crate::config::Protocol::Anthropic
    }
}

/// 检测 stdin 是否为 TTY(终端)。非 TTY 时(管道 / 重定向)子屏回退到 print 输出。
fn atty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// TUI 模式的 tracing writer(F5,2026-09-10 第 25 轮):
/// 把 WARN 级日志改写到 `{根目录}/logs/laew-tui.log`(追加),不再直写 stdout
/// 与全量重绘交错破坏对话区布局。每次写入独立打开文件(日志量低,开销可忽略);
/// 目录创建 / 打开失败时退化 `io::sink`(静默丢弃),绝不影响主流程。
pub fn tui_log_writer() -> impl for<'s> tracing_subscriber::fmt::MakeWriter<'s> {
    struct TuiLogWriter;

    impl<'s> tracing_subscriber::fmt::MakeWriter<'s> for TuiLogWriter {
        type Writer = Box<dyn std::io::Write + 's>;

        fn make_writer(&'s self) -> Self::Writer {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(tui_log_path());
            match file {
                Ok(f) => Box::new(std::io::LineWriter::new(f)),
                Err(_) => Box::new(std::io::sink()),
            }
        }
    }

    TuiLogWriter
}

/// TUI 日志文件路径:`{根目录}/logs/laew-tui.log`(根目录 = 二进制所在目录)。
fn tui_log_path() -> std::path::PathBuf {
    let root = crate::database::paths::Paths::detect()
        .map(|p| p.root_dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let dir = root.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("laew-tui.log")
}

/// 启动 TUI 交互式 REPL
pub async fn run() -> Result<()> {
    run_with_debug(false).await
}

/// 启动 TUI 交互式 REPL;`debug=true` 时开启调试模式(对应 `laew -debug`),
/// 每个用户任务结束后生成 Debug 报告到根目录 `DebugReport/`。
pub async fn run_with_debug(debug: bool) -> Result<()> {
    // TUI 视觉规范(选中态 / 主题色 / 固定底部输入组件的独立配色)依赖颜色输出;
    // 显式覆盖 NO_COLOR 环境变量导致的 crossterm 全局禁色,保证配色可达。
    crossterm::style::Colored::set_ansi_color_disabled(false);

    let mut session = TuiSession::bootstrap_with_debug(debug)?;
    session.print_banner();
    if session.debug.is_some() {
        println!("  [debug] 调试模式已开启,报告将写入根目录 DebugReport/");
        println!();
    }

    if atty() {
        let input_handler = InputHandler::new();
        let mut completion_engine = CompletionEngine::new();

        loop {
            // 每行输入前重扫自定义命令目录(D2):命令文件增删即时生效,无需重启
            completion_engine.reload_custom(&session.paths.work_dir);
            let line = match input_handler.read_line(">> ", &completion_engine)? {
                InputResult::Submitted(l) => l,
                InputResult::Exit => {
                    println!("  再见。");
                    break;
                }
                InputResult::Interrupted => {
                    println!("  (中断) 输入 /exit 或 Ctrl-D 退出。");
                    continue;
                }
            };

            if line.trim().is_empty() {
                continue;
            }

            match session.handle_user_input(&line).await {
                Ok(true) => {
                    // /exit 退出:拆除固定底部输入组件,还原终端滚动区
                    input::teardown_pinned();
                    break;
                }
                Ok(false) => {}
                Err(e) => {
                    input::teardown_pinned();
                    return Err(e);
                }
            }
        }
    } else {
        // 非 TTY:回退到阻塞式 stdin 行读取(用于管道 / e2e)
        use std::io::{self, BufRead};
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if session.handle_user_input(&line).await? {
                break;
            }
        }
    }
    Ok(())
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
                verdict: if qc_pass { Verdict::Pass } else { Verdict::Fail },
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
        assert!(ctx.contains("(质检通过)\n\nsecond answer"), "空行分隔两个 WorkFlow,实际: {ctx}");
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
