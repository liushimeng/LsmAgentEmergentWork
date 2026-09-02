//! TUI 交互界面(rustyline 行式 REPL)。
//!
//! 提供:
//! - 启动横幅展示根目录 / 工作目录 / 当前 provider_name / model_name
//! - 多轮对话(直接输入提示词)
//! - 斜杠命令:/help /provider /model /clear /exit
//! - 斜杠命令自动补全(Tab 补全 + 行内提示)
//! - 文件路径自动补全(Tab 补全目录/文件)

use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::{completion::{Completer, Pair}, hint::Hinter, validate::Validator, highlight::Highlighter, Helper, Context, Editor, DefaultEditor};

use crate::agent::Agent;
use crate::config::{Db, Paths, ProviderRecord};
use crate::llm::{client_from_record, ChatMessage};
use crate::tool::{builtin_registry, builtin_system_prompt};

/// 斜杠命令完整列表(含子命令,用于补全)
const SLASH_COMMANDS: &[&str] = &[
    "help",
    "h",
    "?",
    "exit",
    "quit",
    "q",
    "clear",
    "c",
    "model",
    "provider list",
    "provider ls",
    "provider add",
    "provider use",
    "provider del",
    "provider delete",
    "provider rm",
];

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

// ==================== 自动补全 ====================

/// TUI 辅助器:提供斜杠命令补全 + 路径补全 + 行内提示
struct TuiHelper;

impl Helper for TuiHelper {}

impl Highlighter for TuiHelper {}
impl Validator for TuiHelper {}

impl Completer for TuiHelper {
    type Candidate = Pair;

    fn complete(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> rustyline::Result<(usize, Vec<Pair>)> {
        // 空输入不补全
        if line.is_empty() || pos == 0 {
            return Ok((pos, vec![]));
        }

        // 斜杠命令补全(输入以 '/' 开头)
        if line.starts_with('/') {
            let cmd_matches = complete_slash_command(line, pos);
            if !cmd_matches.is_empty() {
                return Ok((1, cmd_matches)); // 起始位置 1,跳过 '/'
            }
            // 命令无匹配,尝试路径补全(如 /home/...)
            return complete_path(line, pos);
        }

        // 路径模式检测(用于未来工具参数补全)
        if looks_like_path(line) {
            return complete_path(line, pos);
        }

        // 普通提示词不补全
        Ok((pos, vec![]))
    }
}

impl Hinter for TuiHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        // 仅对斜杠命令提供行内提示
        if !line.starts_with('/') || line.len() < 2 || pos < 2 {
            return None;
        }
        // 已有空格(进入子命令/参数阶段)不提示
        if line.contains(' ') {
            return None;
        }
        let input = &line[1..]; // 去掉 '/'
        if input.is_empty() {
            return None;
        }

        // 查找唯一前缀匹配(排除完全匹配)
        let matches: Vec<&&str> = SLASH_COMMANDS
            .iter()
            .filter(|cmd| cmd.starts_with(input) && **cmd != input)
            .collect();

        if matches.len() == 1 {
            Some(matches[0][input.len()..].to_string())
        } else {
            None
        }
    }
}

/// 斜杠命令补全:返回匹配的命令候选列表
fn complete_slash_command(line: &str, _pos: usize) -> Vec<Pair> {
    let input = line[1..].trim(); // 去掉开头的 '/'
    let input_lower = input.to_lowercase();

    SLASH_COMMANDS
        .iter()
        .filter(|cmd| cmd.starts_with(&input_lower) && **cmd != input)
        .map(|cmd| Pair {
            display: format!("/{}", cmd),
            replacement: format!("/{}", cmd),
        })
        .collect()
}

/// 检测输入是否看起来像路径
fn looks_like_path(line: &str) -> bool {
    line.starts_with('/')
        || line.starts_with("./")
        || line.starts_with("../")
        || line.starts_with("~/")
}

/// 路径补全:返回匹配的目录/文件候选列表
fn complete_path(line: &str, _pos: usize) -> rustyline::Result<(usize, Vec<Pair>)> {
    let expanded = expand_tilde(line);

    // 分离目录部分和文件名前缀
    let (dir, prefix) = if expanded.ends_with('/') && expanded.len() > 1 {
        // 以 / 结尾:列出该目录下全部内容
        (expanded.trim_end_matches('/').to_string(), String::new())
    } else if let Some(parent) = Path::new(&expanded).parent() {
        let dir_str = parent.display().to_string();
        let prefix_str = Path::new(&expanded).file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (dir_str, prefix_str)
    } else {
        (expanded.clone(), String::new())
    };

    let dir_path = Path::new(&dir);
    if !dir_path.is_dir() {
        return Ok((line.len(), vec![]));
    }

    let mut pairs: Vec<Pair> = Vec::new();
    if let Ok(entries) = fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with(&prefix) {
                continue;
            }
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            let display_name = if is_dir {
                format!("{}/", name_str)
            } else {
                name_str.to_string()
            };
            // 构造完整替换路径
            let replacement = if dir == "/" {
                format!("/{}", name_str)
            } else if dir == "~" || dir.starts_with("~/") {
                format!("{}/{}", dir, name_str)
            } else {
                format!("{}/{}", dir, name_str)
            };
            pairs.push(Pair {
                display: display_name,
                replacement,
            });
        }
    }

    // 目录优先,按名称排序
    pairs.sort_by(|a, b| {
        let a_is_dir = a.display.ends_with('/');
        let b_is_dir = b.display.ends_with('/');
        match (b_is_dir, a_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.display.cmp(&b.display),
        }
    });

    Ok((line.len(), pairs))
}

/// 展开家目录 '~' → $HOME
fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return path.replacen("~", &home, 1);
        }
    } else if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return home;
        }
    }
    path.to_string()
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
         其他输入             作为提示词进入多轮对话\n\
         \n\
         补全提示:\n\
         - 输入 / 后按 Tab    列出/补全斜杠命令\n\
         - 输入路径时按 Tab   补全目录/文件名\n\
         - 唯一匹配时显示灰色提示,按 → 或 End 接受\n\
         - Ctrl-J 插入换行(多行输入)"
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

    let mut rl = Editor::new()?;
    // 注册自动补全 Helper
    rl.set_helper(Some(TuiHelper));
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
