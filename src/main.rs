//! laew — LsmAgentEmergentWork 命令行入口。

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use lsm_agent::agent::{profile::AgentProfile, Agent};
use lsm_agent::config::{Db, Paths, Protocol};
use lsm_agent::llm::client_from_record;
use lsm_agent::session::Session;

const VERSION_INFO: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (build ",
    env!("LAEW_BUILD_TIME"),
    ", git ",
    env!("LAEW_GIT_HASH"),
    ")",
);

#[derive(Parser, Debug)]
#[command(
    name = "laew",
    bin_name = "laew",
    version = VERSION_INFO,
    about = "LsmAgentEmergentWork - LLM Agent CLI",
    long_about = None,
    disable_help_subcommand = false
)]
struct Cli {
    /// 单轮任务提示词(不进入 TUI)
    #[arg(short = 'p', long = "prompt", value_name = "TEXT", conflicts_with = "file")]
    prompt: Option<String>,

    /// 从文件读取提示词(支持绝对路径和相对路径,与 -p 互斥)
    #[arg(short = 'f', long = "file", value_name = "PATH")]
    file: Option<PathBuf>,

    /// 最大 Agent 迭代次数
    #[arg(long, default_value_t = 16, global = true)]
    max_iterations: usize,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// 管理大模型接入记录(增/删/列/切换)
    #[command(subcommand)]
    Provider(ProviderCmd),
}

#[derive(Subcommand, Debug)]
enum ProviderCmd {
    /// 新增一条接入记录(若库为空则自动激活)
    Add {
        #[arg(long, value_parser = parse_protocol)]
        protocol: Protocol,
        #[arg(long)]
        provider_name: String,
        #[arg(long)]
        model_name: String,
        #[arg(long)]
        end_point: String,
        #[arg(long)]
        api_key: String,
    },
    /// 列出全部接入记录
    List,
    /// 把指定 id 设为当前使用
    Use { id: i64 },
    /// 删除一条接入记录
    Delete { id: i64 },
}

fn parse_protocol(s: &str) -> std::result::Result<Protocol, String> {
    Protocol::parse(s).map_err(|e| e.to_string())
}

fn open_db() -> Result<(Paths, Db)> {
    let paths = Paths::detect().map_err(anyhow::Error::from)?;
    let db = Db::open(&paths).map_err(anyhow::Error::from)?;
    Ok((paths, db))
}

async fn cmd_provider(p: ProviderCmd) -> Result<()> {
    let (_paths, db) = open_db()?;
    match p {
        ProviderCmd::Add { protocol, provider_name, model_name, end_point, api_key } => {
            let id = db
                .add(protocol, &provider_name, &model_name, &end_point, &api_key)
                .map_err(anyhow::Error::from)?;
            println!("✓ 已新增接入记录 id={id}");
        }
        ProviderCmd::List => {
            let records = db.list().map_err(anyhow::Error::from)?;
            if records.is_empty() {
                println!("(空)尚未配置任何接入记录。");
                return Ok(());
            }
            for r in records {
                let marker = if r.is_active { "*" } else { " " };
                println!(
                    "{marker} id={:<3} [{:<9}] {}/{:<24} @ {}  (key 末4位: {})",
                    r.id,
                    r.protocol.as_str(),
                    r.provider_name,
                    r.model_name,
                    r.end_point,
                    tail(&r.api_key, 4)
                );
            }
        }
        ProviderCmd::Use { id } => {
            db.set_active(id).map_err(anyhow::Error::from)?;
            println!("✓ 已切换当前模型为 id={id}");
        }
        ProviderCmd::Delete { id } => {
            db.delete(id).map_err(anyhow::Error::from)?;
            println!("✓ 已删除 id={id}");
        }
    }
    Ok(())
}

fn tail(s: &str, n: usize) -> String {
    s.chars().rev().take(n).collect::<String>().chars().rev().collect()
}

async fn run_one_shot(prompt: String, max_iterations: usize) -> Result<()> {
    let (paths, db) = open_db()?;
    let active = db
        .get_active()
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| anyhow::anyhow!("尚未配置当前模型,请先执行 `laew provider add` 添加接入记录。"))?;
    let profile = AgentProfile::default_profile();
    let user_agent = profile.user_agent();
    let llm = client_from_record(&active, &user_agent).map_err(anyhow::Error::from)?;
    // 在默认系统提示词基础上追加环境上下文(根目录 / 工作目录 / 当前模型)
    let env_tail = format!(
        "\n[环境] 根目录: {}  工作目录: {}  当前模型: [{:}] {}/{}",
        paths.root_dir.display(),
        paths.work_dir.display(),
        active.protocol.as_str(),
        active.provider_name,
        active.model_name
    );
    let profile = profile.with_env_tail(&env_tail);

    let agent = Agent::new(llm, profile).with_max_iterations(max_iterations);
    eprintln!("[laew] 单轮模式: protocol={} provider={} model={}",
        active.protocol.as_str(), active.provider_name, active.model_name);
    // -p 单轮模式每次生成独立 Session
    let mut session = Session::new();
    session.context_mut().push(lsm_agent::llm::ChatMessage::user(prompt));
    let (answer, usage) = agent.run_session(&mut session).await.map_err(anyhow::Error::from)?;
    println!("{answer}");
    if usage.input_tokens > 0 || usage.output_tokens > 0 {
        eprintln!(
            "[laew] 用量: input={}  output={}{}",
            usage.input_tokens,
            usage.output_tokens,
            if usage.cache_read_input_tokens > 0 {
                format!("  cache_read={}", usage.cache_read_input_tokens)
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

/// 从文件读取提示词并执行单轮任务
async fn run_from_file(file_path: PathBuf, max_iterations: usize) -> Result<()> {
    // 相对路径基于工作目录解析
    let absolute_path = if file_path.is_absolute() {
        file_path
    } else {
        std::env::current_dir()?.join(file_path)
    };

    let content = std::fs::read_to_string(&absolute_path)
        .map_err(|e| anyhow::anyhow!("无法读取文件 '{}': {}", absolute_path.display(), e))?;

    let content = content.trim().to_string();
    if content.is_empty() {
        anyhow::bail!("文件 '{}' 内容为空", absolute_path.display());
    }

    eprintln!("[laew] 从文件读取提示词: {} ({} 字符)", absolute_path.display(), content.len());
    run_one_shot(content, max_iterations).await
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    match cli.cmd {
        Some(Cmd::Provider(p)) => cmd_provider(p).await,
        None => {
            if let Some(prompt) = cli.prompt {
                run_one_shot(prompt, cli.max_iterations).await
            } else if let Some(file_path) = cli.file {
                run_from_file(file_path, cli.max_iterations).await
            } else {
                lsm_agent::tui::run().await
            }
        }
    }
}

