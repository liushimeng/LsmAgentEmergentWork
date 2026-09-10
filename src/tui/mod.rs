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

pub mod commands;
pub mod completion;
pub mod engine;
pub mod export;
pub mod form;
pub mod render;
pub mod screen;
pub mod theme;

mod input;

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
    pub fn reset_session(&mut self) {
        self.session = Session::new();
        self.transcript.clear();
        self.session_usage = crate::llm::Usage::default();
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
        // 普通提示词:Orchestrator 编排(可取消:Ctrl-C 经 SIGINT 自动感知,零新增命令)
        self.session.context_mut().push(ChatMessage::user(prompt));
        // debug 模式:每个任务开始前重置采集器
        if let Some(collector) = &self.debug {
            collector.reset(self.session.id());
        }
        println!("  [orchestrator 调度中... Ctrl-C 取消]");
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
            .handle_cancellable(&mut self.session, &cancel)
            .await;
        // 任务结束:撤掉 SIGINT 监听,避免游离监听吞掉后续按键窗口外的信号
        sig_task.abort();
        // debug 模式:任务结束(无论成败)后生成 Debug 报告
        if let (Some(collector), Some(raw_llm)) = (&self.debug, &self.debug_llm_raw) {
            self.emit_debug_report(collector, raw_llm.clone(), prompt, &handle_result)
                .await;
        }
        // 终态:屏幕输出 + transcript 记录(outcome, response, 本轮用量)
        let entry = match &handle_result {
            Ok(OrchestrationOutcome::DirectAnswer { text, usage, .. }) => {
                self.print_assistant_text_with_agent("Yolo", text, usage);
                Some((OutcomeKind::DirectAnswer, text.clone(), *usage))
            }
            Ok(OrchestrationOutcome::Executed { result }) => {
                let text = format_task_result(result);
                self.print_task_result(result);
                Some((OutcomeKind::Executed, text, result.total_usage))
            }
            Ok(OrchestrationOutcome::Failed {
                suggestion, usage, ..
            }) => {
                println!();
                println!("  [agent failed]");
                println!("  建议: {suggestion}");
                // 2026-09-10 第 22 轮:与 Executed/DirectAnswer 路径对齐,统一调用
                // print_usage 输出(input/output + cache_read + cache_creation),
                // 避免 Failed 路径用量维度缺失,导致用户看不到 prompt caching 命中量。
                self.print_usage(usage);
                Some((
                    OutcomeKind::Failed,
                    format!("执行失败。\n建议: {suggestion}"),
                    *usage,
                ))
            }
            Err(e) if matches!(e, crate::error::AgentError::Cancelled) => {
                println!();
                println!("  ✓ 本次任务已取消,可继续输入新指令。");
                Some((
                    OutcomeKind::Cancelled,
                    "(任务已取消)".to_string(),
                    crate::llm::Usage::default(),
                ))
            }
            Err(e) => {
                eprintln!("  [agent error] {e}");
                Some((
                    OutcomeKind::Error,
                    format!("[agent error] {e}"),
                    crate::llm::Usage::default(),
                ))
            }
        };
        if let Some((outcome, response, usage)) = entry {
            self.session_usage = merge_usage(self.session_usage, usage);
            self.transcript.push(TranscriptEntry {
                ts: turn_ts,
                raw_input: raw.to_string(),
                prompt: prompt.to_string(),
                response,
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
            Ok(path) => println!("  [debug] 报告已生成: {}", path.display()),
            Err(e) => eprintln!("  [debug] 报告生成失败: {e}"),
        }
    }

    fn print_assistant_text_with_agent(&self, agent_name: &str, text: &str, usage: &crate::llm::Usage) {
        if !text.is_empty() {
            println!();
            println!("  [agent: {agent_name}]");
            self.print_with_optional_highlight(text);
            println!();
        } else {
            println!("  (模型未返回文本)");
        }
        self.print_usage(usage);
    }

    fn print_task_result(&self, result: &crate::agent::orchestrator::TaskResult) {
        for line in format_task_result(result).lines() {
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
                print!(
                    "\x1b[38;5;{}m{}{}\x1b[0m",
                    color_to_ansi256(span.fg),
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
                    print!(
                        "\x1b[38;5;{}m{}{}\x1b[0m",
                        color_to_ansi256(span.fg),
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

    fn print_usage(&self, usage: &crate::llm::Usage) {
        if usage.input_tokens > 0 || usage.output_tokens > 0 {
            let mut cache = String::new();
            if usage.cache_read_input_tokens > 0 {
                cache.push_str(&format!("  cache_read={}", usage.cache_read_input_tokens));
            }
            if usage.cache_creation_input_tokens > 0 {
                cache.push_str(&format!("  cache_creation={}", usage.cache_creation_input_tokens));
            }
            println!(
                "  本次用量: input={}  output={}{}",
                usage.input_tokens, usage.output_tokens, cache
            );
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
                self.reset_session();
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
            }
            "new" | "n" => {
                self.reset_session();
                if atty() {
                    print!("\x1b[2J\x1b[H");
                    self.print_banner();
                } else {
                    println!("  已开启新会话, Session ID: {}", self.session.id);
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
                        "  ✓ 已导出 {fmt_name}: {} ({} 轮对话)",
                        path.display(),
                        meta.turns
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
        let result = Self::run_screen_loop(screen).await;
        leave_alt().map_err(anyhow::Error::from)?;
        // 重建 YoloRunner(可能切换了 use)
        self.rebuild_orchestrator()?;
        result
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
        let result = Self::run_screen_loop(screen).await;
        leave_alt().map_err(anyhow::Error::from)?;
        self.rebuild_orchestrator()?;
        result
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
        let result = Self::run_screen_loop(screen).await;
        leave_alt().map_err(anyhow::Error::from)?;
        self.rebuild_orchestrator()?;
        result
    }

    /// 通用子屏循环:渲染 → 读键 → 处理 Outcome。
    /// 屏幕栈:`Vec<Box<dyn Screen>>`,top 是当前屏;`Push` 压栈、`Pop` 出栈。
    /// 当栈清空时退出循环(回到主屏)。
    async fn run_screen_loop(initial: Box<dyn crate::tui::engine::Screen>) -> Result<()> {
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
                    // Toast:弹出现有屏栈,在主屏展示消息。
                    // 适合"操作成功/失败"的反馈,用户能立即看到。
                    while let Some(mut s) = stack.pop() {
                        s.on_exit();
                    }
                    println!("  {msg}");
                    break;
                }
                Outcome::Quit => std::process::exit(0),
            }
        }
        Ok(())
    }
}

/// 格式化任务执行结果为多行文本(D8:屏幕打印与导出同源,避免两处漂移)。
fn format_task_result(result: &crate::agent::orchestrator::TaskResult) -> String {
    let mut out = String::new();
    let plan_doc_display = result
        .plan_doc
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "无".into());
    out.push_str(&format!(
        "  [task executed: difficulty={}, plan_doc={}, workflows={}]\n",
        result.classification.task_level.display_name(),
        plan_doc_display,
        result.workflows.len()
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
        out.push_str(&format!(
            "  本次用量: input={}  output={}{}\n",
            usage.input_tokens, usage.output_tokens, cache
        ));
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
