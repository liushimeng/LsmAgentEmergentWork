//! TUI 交互界面 —— 独立 CLI 渲染引擎 + 斜杠命令 + 多轮对话。
//!
//! 架构见 `docs/TUI界面与CLI渲染引擎/02-技术设计.md`。
//! - 主屏:保留 0.1.2 的 `InputHandler` 单行输入 + 斜杠命令补全。
//! - 子屏:`engine.rs` 的 Screen 栈,接管 `/provider *` 系列。
//! - `/provider` 单独输入默认路由到 `/provider list`。

use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;

use crate::agent::{profile::AgentProfile, Agent};
use crate::config::{Db, Paths, ProviderRecord};
use crate::llm::{client_from_record, ChatMessage};
use crate::session::Session;

pub mod completion;
pub mod engine;
pub mod form;
pub mod screen;
pub mod theme;

mod input;

use completion::CompletionEngine;
use input::{InputHandler, InputResult};

pub struct TuiSession {
    pub paths: Paths,
    pub db: Arc<Mutex<Db>>,
    pub agent: Agent,
    pub session: Session,
}

impl TuiSession {
    pub fn bootstrap() -> Result<Self> {
        let paths = Paths::detect().map_err(anyhow::Error::from)?;
        let db = Db::open(&paths).map_err(anyhow::Error::from)?;
        let db = Arc::new(Mutex::new(db));
        let agent = build_agent_with_active(&db)?;
        let session = Session::new();
        Ok(Self { paths, db, agent, session })
    }

    pub fn print_banner(&self) {
        let active = self.db.lock().expect("db").get_active().ok().flatten();
        println!("╔══════════════════════════════════════════════════════════╗");
        println!("║  LsmAgentEmergentWork  ·  laew  TUI  ·  v{}           ║", env!("CARGO_PKG_VERSION"));
        println!("║  编译时间: {}                          ║", env!("LAEW_BUILD_TIME"));
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║  根目录 : {:<46} ║", truncate(&self.paths.root_dir.display().to_string(), 46));
        println!("║  工作目录: {:<45} ║", truncate(&self.paths.work_dir.display().to_string(), 45));
        match active {
            Some(r) => println!(
                "║  当前模型: [{}] {}/{} {:<25} ║",
                r.protocol.as_str(),
                r.provider_name,
                r.model_name,
                truncate(&format!("@ {}", r.end_point), 25)
            ),
            None => println!("║  当前模型: <未配置, 使用 /provider add 添加>{:<14} ║", ""),
        }
        println!("║  Session: {:<46} ║", truncate(&self.session.id, 46));
        println!("╚══════════════════════════════════════════════════════════╝");
        println!("  输入提示词开始对话, 输入 / 查看可用命令。");
        println!("  快捷键: ↑↓ 选择补全  Enter 提交  Esc 关闭补全  Ctrl-D 退出");
        println!();
    }

    /// 切换当前 provider(根据 id),并重新构造 Agent
    pub fn switch_provider(&mut self, id: i64) -> Result<()> {
        let record = self.db.lock().expect("db").get(id).map_err(anyhow::Error::from)?;
        self.db.lock().expect("db").set_active(id).map_err(anyhow::Error::from)?;
        let user_agent = self.agent.profile().user_agent();
        self.agent = build_agent_with_record(&record, &user_agent)?;
        Ok(())
    }

    /// 重置会话:清空上下文并生成新 Session ID。
    pub fn reset_session(&mut self) {
        self.session = Session::new();
    }

    pub fn add_provider_interactive(&self) -> Result<i64> {
        println!("  新增接入记录:");
        let protocol = read_line_prompt("    protocol (anthropic/openai): ")?;
        let protocol = crate::config::Protocol::parse(protocol.trim())
            .map_err(anyhow::Error::from)?;
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
        let records = self.db.lock().expect("db").list().map_err(anyhow::Error::from)?;
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
        if let Some(rest) = line.strip_prefix('/') {
            // 斜杠命令
            return self.handle_slash(rest).await;
        }
        // 普通提示词
        self.session.context_mut().push(ChatMessage::user(line));
        match self.agent.run_session(&mut self.session).await {
            Ok((text, usage)) => {
                if !text.is_empty() {
                    println!();
                    println!("  [assistant]");
                    for line in text.lines() {
                        println!("  {line}");
                    }
                    println!();
                } else {
                    println!("  (模型未返回文本)");
                }
                if usage.input_tokens > 0 || usage.output_tokens > 0 {
                    let cache = if usage.cache_read_input_tokens > 0 {
                        format!("  cache_read={}", usage.cache_read_input_tokens)
                    } else {
                        String::new()
                    };
                    println!(
                        "  本次用量: input={}  output={}{}",
                        usage.input_tokens, usage.output_tokens, cache
                    );
                }
            }
            Err(e) => {
                eprintln!("  [agent error] {e}");
            }
        }
        Ok(false)
    }

    async fn handle_slash(&mut self, cmd: &str) -> Result<bool> {
        let mut it = cmd.split_whitespace();
        let head = it.next().unwrap_or("");
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
                println!("  已清空对话历史并开启新会话, Session ID: {}", self.session.id);
            }
            "new" | "n" => {
                self.reset_session();
                println!("  已开启新会话, Session ID: {}", self.session.id);
            }
            "model" => {
                if let Some(r) = self.db.lock().expect("db").get_active().map_err(anyhow::Error::from)? {
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
            "" => {}
            other => {
                println!("  未知斜杠命令: /{other}");
                let suggestions = suggest_similar_commands(other);
                if !suggestions.is_empty() {
                    println!("  您是否想输入: {}", suggestions.join(", "));
                }
                println!("  输入 /help 查看所有命令。");
            }
        }
        Ok(false)
    }

    /// 进入 ProviderList 屏(子屏通过 engine 渲染)。
    /// 当 stdin 不是 TTY(如 e2e 管道)时,回退到 print 输出以保持兼容。
    async fn run_provider_list_screen(&mut self) -> Result<()> {
        use crate::tui::engine::{enter_alt, leave_alt, present, read_key, Outcome};
        use crate::tui::screen::provider_list::ProviderList;

        if !atty() {
            // 非 TTY:回退到 print 输出
            return self.list_providers();
        }

        let screen = ProviderList::new(self.db.clone(), self.paths.clone());
        enter_alt().map_err(anyhow::Error::from)?;
        let result = Self::run_screen_loop(screen).await;
        leave_alt().map_err(anyhow::Error::from)?;
        // 重建 Agent(可能切换了 use)
        self.agent = build_agent_with_active(&self.db)?;
        result
    }

    async fn run_provider_add_screen(&mut self) -> Result<()> {
        use crate::tui::engine::{enter_alt, leave_alt, present, read_key, Outcome};
        use crate::tui::screen::provider_form::ProviderForm;

        if !atty() {
            return self.add_provider_interactive().map(|_| ());
        }

        let db = self.db.clone();
        let on_done = Box::new(move |id: i64| {
            let _ = db.lock().expect("db").set_active(id);
        });
        let screen = ProviderForm::new_add(self.db.clone(), self.paths.clone(), on_done);
        enter_alt().map_err(anyhow::Error::from)?;
        let result = Self::run_screen_loop(screen).await;
        leave_alt().map_err(anyhow::Error::from)?;
        self.agent = build_agent_with_active(&self.db)?;
        result
    }

    async fn run_provider_del_screen(&mut self) -> Result<()> {
        use crate::tui::engine::{enter_alt, leave_alt, present, read_key, Outcome};
        use crate::tui::screen::provider_del::ProviderDelPicker;

        if !atty() {
            // 非 TTY:提示使用 CLI 子命令
            println!("  非交互模式请使用: laew provider del <id>");
            return Ok(());
        }

        let screen = ProviderDelPicker::new(self.db.clone(), self.paths.clone(), -1);
        enter_alt().map_err(anyhow::Error::from)?;
        let result = Self::run_screen_loop(screen).await;
        leave_alt().map_err(anyhow::Error::from)?;
        self.agent = build_agent_with_active(&self.db)?;
        result
    }

    /// 通用子屏循环:渲染 → 读键 → 处理 Outcome。
    async fn run_screen_loop(mut screen: impl crate::tui::engine::Screen) -> Result<()> {
        use crate::tui::engine::{present, read_key, Outcome, Rect, Frame};

        screen.on_enter();
        loop {
            let area = Rect::full_screen();
            let mut frame = Frame::new(area);
            screen.render(&mut frame);
            present(&frame).map_err(anyhow::Error::from)?;

            let key = read_key().map_err(anyhow::Error::from)?;
            match screen.handle_key(key) {
                Outcome::Continue => {}
                Outcome::Pop | Outcome::Toast(_) => break,
                Outcome::Push(_) => {
                    // 简化:不支持嵌套 push;当作 Pop 处理(后续可扩展为栈)
                    break;
                }
                Outcome::Quit => std::process::exit(0),
            }
        }
        screen.on_exit();
        Ok(())
    }
}

/// 建议相似命令（简单的编辑距离近似）。
fn suggest_similar_commands(input: &str) -> Vec<String> {
    let all_commands = [
        "help", "h", "?",
        "exit", "quit", "q",
        "clear", "c",
        "new", "n",
        "model",
        "provider", "provider list", "provider add", "provider use", "provider del",
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
    if a_len == 0 { return b_len; }
    if b_len == 0 { return a_len; }

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

/// 截断字符串到指定显示宽度（简化版，按字符数）。
fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_len {
        s.to_string()
    } else {
        chars[..max_len - 1].iter().collect::<String>() + "…"
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
        "  {} id={} [{:>9}] {}/{} @ {}  key=****{}  ({})",
        marker,
        r.id,
        r.protocol.as_str(),
        r.provider_name,
        r.model_name,
        r.end_point,
        r.api_key.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>(),
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
}

/// 启动活跃 agent;若未配置,使用占位提示信息
fn build_agent_with_active(db: &Arc<Mutex<Db>>) -> Result<Agent> {
    let profile = AgentProfile::default_profile();
    let user_agent = profile.user_agent();
    match db.lock().expect("db").get_active().map_err(anyhow::Error::from)? {
        Some(r) => build_agent_with_record(&r, &user_agent),
        None => {
            let system = profile.system_prompt
                + "\n[系统] 当前尚未配置大模型接入记录。请先使用 `laew provider add` 或 TUI 内 `/provider add` 完成配置后再开始对话。";
            let profile = AgentProfile::new(&profile.name, system);
            Ok(Agent::new(Arc::new(NoopLlm), profile))
        }
    }
}

fn build_agent_with_record(r: &ProviderRecord, user_agent: &str) -> Result<Agent> {
    let llm = client_from_record(r, user_agent).map_err(anyhow::Error::from)?;
    Ok(Agent::new(llm, AgentProfile::default_profile()))
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
}

/// 检测 stdin 是否为 TTY(终端)。非 TTY 时(管道 / 重定向)子屏回退到 print 输出。
fn atty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// 启动 TUI 交互式 REPL
pub async fn run() -> Result<()> {
    let mut session = TuiSession::bootstrap()?;
    session.print_banner();

    if atty() {
        let input_handler = InputHandler::new();
        let completion_engine = CompletionEngine::new();

        loop {
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

            if session.handle_user_input(&line).await? {
                break;
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