//! TUI 交互界面(rustyline 行式 REPL)。
//!
//! 提供:
//! - 启动横幅展示根目录 / 工作目录 / 当前 provider_name / model_name
//! - 多轮对话(直接输入提示词)
//! - 斜杠命令:/help /provider /model /clear /exit

use std::sync::Arc;

use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::agent::Agent;
use crate::config::{Db, Paths, ProviderRecord};
use crate::llm::{client_from_record, ChatMessage};
use crate::tool::{builtin_registry, builtin_system_prompt};

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
        println!("╔══════════════════════════════════════════════════════════");
        println!("║  LsmAgentEmergentWork  ·  laew  TUI  ·  v{}", env!("CARGO_PKG_VERSION"));
        println!("║  编译时间: {}", env!("LAEW_BUILD_TIME"));
        println!("║  根目录 : {}", self.paths.root_dir.display());
        println!("║  工作目录: {}", self.paths.work_dir.display());
        println!("║  数据库  : {}", self.paths.db_path.display());
        match active {
            Some(r) => println!(
                "║  当前模型: [{}] {} / {}  @ {}",
                r.protocol.as_str(),
                r.provider_name,
                r.model_name,
                r.end_point
            ),
            None => println!("║  当前模型: <未配置,使用 /provider add 添加>"),
        }
        println!("╚══════════════════════════════════════════════════════════");
        println!("输入提示词开始对话;斜杠命令 /help 查看帮助。");
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
        let rl = &mut readline_single("")?;
        let protocol = read_line_interactive(rl, "protocol (anthropic/openai): ")?;
        let protocol = crate::config::Protocol::parse(protocol.trim())
            .map_err(anyhow::Error::from)?;
        let provider_name = read_line_interactive(rl, "provider_name: ")?;
        let model_name = read_line_interactive(rl, "model_name: ")?;
        let end_point = read_line_interactive(rl, "end_point: ")?;
        let api_key = read_line_interactive(rl, "api_key: ")?;
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
        println!("✓ 已新增接入记录 id={id}");
        Ok(id)
    }

    pub fn list_providers(&self) -> Result<()> {
        let records = self.db.list().map_err(anyhow::Error::from)?;
        if records.is_empty() {
            println!("(空)尚未配置任何接入记录。");
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
                    println!("[assistant]");
                    println!("{text}");
                    println!();
                } else {
                    println!("(模型未返回文本)");
                }
            }
            Err(e) => {
                eprintln!("[agent error] {e}");
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
                return Ok(true);
            }
            "clear" | "c" => {
                self.history.clear();
                println!("已清空当前对话历史。");
            }
            "model" => {
                if let Some(r) = self.db.get_active().map_err(anyhow::Error::from)? {
                    println!(
                        "[{}] {} / {}  @ {}",
                        r.protocol.as_str(),
                        r.provider_name,
                        r.model_name,
                        r.end_point
                    );
                } else {
                    println!("当前未配置模型。");
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
                                println!("✓ 已切换到 id={id}");
                            }
                        }
                    }
                    "use" => {
                        let id_str = it.next().unwrap_or("");
                        if let Ok(id) = id_str.parse::<i64>() {
                            self.switch_provider(id)?;
                            println!("✓ 已切换到 id={id}");
                        } else {
                            println!("用法: /provider use <id>");
                        }
                    }
                    "del" | "delete" | "rm" => {
                        let id_str = it.next().unwrap_or("");
                        if let Ok(id) = id_str.parse::<i64>() {
                            self.db.delete(id).map_err(anyhow::Error::from)?;
                            println!("✓ 已删除 id={id}");
                        } else {
                            println!("用法: /provider del <id>");
                        }
                    }
                    "" => {
                        println!("/provider 子命令: add | list | use <id> | del <id>");
                    }
                    other => println!("未知 /provider 子命令: {other}"),
                }
            }
            "" => {}
            other => println!("未知斜杠命令: /{other},输入 /help 查看。"),
        }
        Ok(false)
    }
}

fn readline_single(prompt: &str) -> Result<DefaultEditor> {
    let mut rl = DefaultEditor::new()?;
    if !prompt.is_empty() {
        let _ = rl.readline(prompt)?;
    }
    Ok(rl)
}

fn read_line_interactive(rl: &mut DefaultEditor, prompt: &str) -> Result<String> {
    let line = rl.readline(prompt)?;
    Ok(line)
}

fn print_record(r: &ProviderRecord) {
    let marker = if r.is_active { "*" } else { " " };
    println!(
        "{} id={} [{:>9}] {}/{} @ {}  key=****{}  ({})",
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
    println!(
        "可用命令:\n\
         /help, /h, /?        显示本帮助\n\
         /provider list       列出所有接入记录\n\
         /provider add        交互式新增接入记录\n\
         /provider use <id>   切换当前模型\n\
         /provider del <id>   删除接入记录\n\
         /model               显示当前模型\n\
         /clear, /c           清空当前对话历史\n\
         /exit, /quit, /q     退出\n\
         其他输入             作为提示词进入多轮对话"
    );
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
            text: "尚未配置大模型接入记录,请先使用 `laew provider add` 或 TUI 内 `/provider add` 完成配置。".to_string(),
            tool_calls: vec![],
        })
    }
}

/// 启动 TUI 交互式 REPL
pub async fn run() -> Result<()> {
    let mut session = TuiSession::bootstrap()?;
    session.print_banner();

    let mut rl = DefaultEditor::new()?;
    loop {
        let prompt = ">> ";
        let line = match rl.readline(prompt) {
            Ok(l) => l,
            Err(ReadlineError::Interrupted) => {
                println!("(Ctrl-C) 输入 /exit 退出。");
                continue;
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("读取错误: {e}");
                break;
            }
        };
        let _ = rl.add_history_entry(&line);
        if session.handle_user_input(&line).await? {
            break;
        }
    }
    Ok(())
}
