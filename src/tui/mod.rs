//! TUI 交互界面（基于 crossterm 的自定义输入处理器）。
//!
//! 提供:
//! - 启动横幅展示根目录 / 工作目录 / 当前 provider_name / model_name
//! - 多轮对话(直接输入提示词)
//! - 斜杠命令: /help /provider /model /clear /exit
//! - 下拉式斜杠命令补全（上下箭头导航, Enter/Tab 接受, Esc 关闭）
//! - 未选中项显示灰色（未确认状态）

use std::sync::Arc;

use anyhow::Result;

use crate::agent::Agent;
use crate::config::{Db, Paths, ProviderRecord};
use crate::llm::{client_from_record, ChatMessage};
use crate::tool::{builtin_registry, builtin_system_prompt};

pub mod completion;
pub mod input;

use completion::CompletionEngine;
use input::{InputHandler, InputResult};

pub struct TuiSession {
    pub paths: Paths,
    pub db: Db,
    pub agent: Agent,
    pub history: Vec<ChatMessage>,
}

impl TuiSession {
    pub fn bootstrap() -> Result<Self> {
        let paths = Paths::detect().map_err(anyhow::Error::from)?;
        let db = Db::open(&paths).map_err(anyhow::Error::from)?;
        let (agent, history) = build_agent_with_active(&db)?;
        Ok(Self { paths, db, agent, history })
    }

    pub fn print_banner(&self) {
        let active = self.db.get_active().ok().flatten();
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
        println!("╚══════════════════════════════════════════════════════════╝");
        println!("  输入提示词开始对话, 输入 / 查看可用命令。");
        println!("  快捷键: ↑↓ 选择补全  Enter 提交  Esc 关闭补全  Ctrl-D 退出");
        println!();
    }

    /// 切换当前 provider(根据 id),并重新构造 Agent
    pub fn switch_provider(&mut self, id: i64) -> Result<()> {
        let record = self.db.get(id).map_err(anyhow::Error::from)?;
        self.db.set_active(id).map_err(anyhow::Error::from)?;
        let (agent, history) = build_agent_with_record(record)?;
        self.agent = agent;
        self.history = history;
        Ok(())
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
        let records = self.db.list().map_err(anyhow::Error::from)?;
        if records.is_empty() {
            println!("  (空)尚未配置任何接入记录。");
            return Ok(());
        }
        for r in records {
            print_record(&r);
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
        self.history.push(ChatMessage::user(line));
        match self.agent.run_with_history(&mut self.history).await {
            Ok(text) => {
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
                self.history.clear();
                println!("  已清空当前对话历史。");
            }
            "model" => {
                if let Some(r) = self.db.get_active().map_err(anyhow::Error::from)? {
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
                    "list" | "ls" => self.list_providers()?,
                    "add" => {
                        let id = self.add_provider_interactive()?;
                        // 首次添加会自动激活;若想立即启用
                        if let Ok(Some(r)) = self.db.get_active() {
                            if r.id == id {
                                self.switch_provider(id)?;
                                println!("  ✓ 已切换到 id={id}");
                            }
                        }
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
                        let id_str = it.next().unwrap_or("");
                        if let Ok(id) = id_str.parse::<i64>() {
                            self.db.delete(id).map_err(anyhow::Error::from)?;
                            println!("  ✓ 已删除 id={id}");
                        } else {
                            println!("  用法: /provider del <id>");
                        }
                    }
                    "" => {
                        println!("/provider 子命令:");
                        println!("  add          交互式新增接入记录");
                        println!("  list         列出所有接入记录");
                        println!("  use <id>     切换当前模型");
                        println!("  del <id>     删除接入记录");
                    }
                    other => println!("  未知 /provider 子命令: {other}"),
                }
            }
            "" => {}
            other => {
                println!("  未知斜杠命令: /{other}");
                // 给出相似命令建议
                let suggestions = suggest_similar_commands(other);
                if !suggestions.is_empty() {
                    println!("  您是否想输入: {}", suggestions.join(", "));
                }
                println!("  输入 /help 查看所有命令。");
            }
        }
        Ok(false)
    }
}

/// 建议相似命令（简单的编辑距离近似）。
fn suggest_similar_commands(input: &str) -> Vec<String> {
    let all_commands = [
        "help", "h", "?",
        "exit", "quit", "q",
        "clear", "c",
        "model",
        "provider", "provider list", "provider add", "provider use", "provider del",
    ];
    let input_lower = input.to_lowercase();
    all_commands
        .iter()
        .filter(|cmd| {
            // 简单启发：前缀重叠或包含关系
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
    println!("  │  /clear (c)        清空当前对话历史                       │");
    println!("  │  /model            显示当前模型                           │");
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
fn build_agent_with_active(db: &Db) -> Result<(Agent, Vec<ChatMessage>)> {
    match db.get_active().map_err(anyhow::Error::from)? {
        Some(r) => build_agent_with_record(r),
        None => {
            let tools = builtin_registry();
            let system = builtin_system_prompt()
                + "\n[系统] 当前尚未配置大模型接入记录。请先使用 `laew provider add` 或 TUI 内 `/provider add` 完成配置后再开始对话。";
            let agent = Agent::new(Arc::new(NoopLlm), tools, system);
            Ok((agent, Vec::new()))
        }
    }
}

fn build_agent_with_record(r: ProviderRecord) -> Result<(Agent, Vec<ChatMessage>)> {
    let llm = client_from_record(&r).map_err(anyhow::Error::from)?;
    let tools = builtin_registry();
    let agent = Agent::new(llm, tools, builtin_system_prompt());
    Ok((agent, Vec::new()))
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
    ) -> crate::error::Result<crate::llm::Completion> {
        Ok(crate::llm::Completion {
            text: "尚未配置大模型接入记录, 请先使用 `laew provider add` 或 TUI 内 `/provider add` 完成配置。".to_string(),
            tool_calls: vec![],
        })
    }
}

/// 启动 TUI 交互式 REPL
pub async fn run() -> Result<()> {
    let mut session = TuiSession::bootstrap()?;
    session.print_banner();

    let input_handler = InputHandler::new();
    let completion_engine = CompletionEngine::new();

    loop {
        // 使用自定义输入处理器读取一行
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

        // 处理输入
        if session.handle_user_input(&line).await? {
            break;
        }
    }
    Ok(())
}
