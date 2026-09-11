//! TUI 交互界面 —— 独立 CLI 渲染引擎 + 斜杠命令 + 多轮对话。
//!
//! 架构见 `docs/TUI界面与CLI渲染引擎/02-技术设计.md`。
//! - 主屏:保留 0.1.2 的 `InputHandler` 单行输入 + 斜杠命令补全。
//! - 子屏:`engine.rs` 的 Screen 栈,接管 `/provider *` 系列。
//! - `/provider` 单独输入默认路由到 `/provider list`。
//!
//! 2026-09-11 按单文件 ≤1800 行规范拆分(方案 tmpPlan/2026-09-11_代码文件1800行上限模块化拆分方案.md):
//! - `dispatch.rs` 输入分发与任务结果输出(handle_user_input / dispatch_prompt / print_* 家族)
//! - `slash.rs` 斜杠命令路由与 run_* 处理器
//! - `provider_screen.rs` /provider 子屏桥接 + 通用 Screen 栈循环
//! - `format.rs` 纯函数格式化辅助(任务结果双版本 / 截断系 / 帮助打印等)
//! - `theme.rs` ANSI 颜色集中管理(吸收 attrs / bg / color → ANSI 转换)
//! 本文件保留会话外壳(TuiSession 生命周期 + orchestrator 装配 + run() 入口)。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;

use crate::agent::debug::{DebugCollector, DebugLlmClient};
use crate::agent::orchestrator::MultiAgentOrchestrator;
use crate::config::{Db, Paths};
use crate::llm::{client_from_record, ChatMessage};
use crate::session::Session;

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

mod dispatch;
mod format;
mod input;
mod provider_screen;
mod slash;

use branches::BranchStore;
use completion::CompletionEngine;
use export::TranscriptEntry;
use format::{fit_display, print_record, read_line_prompt};
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

/// 清理外部工具/模型输出中的终端控制序列,供 `-p` 与 TUI 共用。
pub fn sanitize_terminal_controls(input: &str) -> String {
    format::sanitize_terminal_controls(input)
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
        let active = self
            .db
            .lock()
            .expect("db")
            .get_active_or_env()
            .ok()
            .flatten();
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
            fit_display(
                &format!("{}  (切换 /theme [kind])", active_theme.as_str()),
                46
            )
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
