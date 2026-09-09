//! laew — LsmAgentEmergentWork 命令行入口。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};

use lsm_agent::agent::orchestrator::{MultiAgentOrchestrator, OrchestrationOutcome};
use lsm_agent::agent::profile::AgentProfile;
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
    #[arg(
        short = 'p',
        long = "prompt",
        value_name = "TEXT",
        conflicts_with = "file"
    )]
    prompt: Option<String>,

    /// 从文件读取提示词(支持绝对路径和相对路径,与 -p 互斥)
    #[arg(short = 'f', long = "file", value_name = "PATH")]
    file: Option<PathBuf>,

    /// 从 JSON 配置文件批量导入 Provider(支持绝对路径和相对路径)
    #[arg(long = "inprovider", value_name = "PATH", conflicts_with_all = ["prompt", "file", "outprovider"])]
    inprovider: Option<PathBuf>,

    /// 导出所有 Provider 到 JSON 配置文件(支持绝对路径和相对路径)
    #[arg(long = "outprovider", value_name = "PATH", conflicts_with_all = ["prompt", "file", "inprovider"])]
    outprovider: Option<PathBuf>,

    /// 最大 Agent 迭代次数
    #[arg(long, default_value_t = 16, global = true)]
    max_iterations: usize,

    /// 调试模式:采集各 Agent 输入/输出/性能/质量,任务结束后由 Debug Agent 评估,
    /// 报告写入根目录 DebugReport/(也支持 `-debug` 写法)
    #[arg(long = "debug", global = true)]
    debug: bool,

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
        /// 上下文最大 Token 数(默认 800K;支持 800000/800K/1M 写法;0 = 不限制,关闭自动压缩)
        #[arg(long, value_parser = parse_context_size)]
        context_max_size: Option<u64>,
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

fn parse_context_size(s: &str) -> std::result::Result<u64, String> {
    lsm_agent::config::parse_context_size(s)
}

fn open_db() -> Result<(Paths, Db)> {
    let paths = Paths::detect().map_err(anyhow::Error::from)?;
    let db = Db::open(&paths).map_err(anyhow::Error::from)?;
    Ok((paths, db))
}

async fn cmd_provider(p: ProviderCmd) -> Result<()> {
    let (_paths, db) = open_db()?;
    match p {
        ProviderCmd::Add {
            protocol,
            provider_name,
            model_name,
            end_point,
            api_key,
            context_max_size,
        } => {
            let id = db
                .add_with_context(
                    protocol,
                    &provider_name,
                    &model_name,
                    &end_point,
                    &api_key,
                    context_max_size,
                )
                .map_err(anyhow::Error::from)?;
            println!(
                "✓ 已新增接入记录 id={id}(context_max_size={})",
                lsm_agent::config::format_context_size(
                    context_max_size.unwrap_or(lsm_agent::config::DEFAULT_CONTEXT_MAX_SIZE)
                )
            );
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
                    "{marker} id={:<3} [{:<9}] {}/{:<24} @ {}  (key 末4位: {}, ctx: {})",
                    r.id,
                    r.protocol.as_str(),
                    r.provider_name,
                    r.model_name,
                    r.end_point,
                    tail(&r.api_key, 4),
                    lsm_agent::config::format_context_size(r.context_max_size)
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
    s.chars()
        .rev()
        .take(n)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

async fn run_one_shot(
    prompt: String,
    max_iterations: usize,
    debug: bool,
    mode: &str,
) -> Result<()> {
    let (paths, db) = open_db()?;
    let active = db
        .get_active()
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| {
            anyhow::anyhow!("尚未配置当前模型,请先执行 `laew provider add` 添加接入记录。")
        })?;
    // 构造期 User-Agent 仅作兜底默认值:正常运行时 Agent 循环按当前 profile
    // 逐请求覆盖(RequestMeta::user_agent,第 08 轮),8 角色在抓包层面各自可辨识
    let work_profile = AgentProfile::work_profile();
    let user_agent = work_profile.user_agent();
    let llm = client_from_record(&active, &user_agent).map_err(anyhow::Error::from)?;

    eprintln!(
        "[laew] 单轮模式: protocol={} provider={} model={}{}",
        active.protocol.as_str(),
        active.provider_name,
        active.model_name,
        if debug { " [debug]" } else { "" }
    );

    // 构造 MultiAgentOrchestrator(6 角色);debug 模式下包装饰器并注入采集器
    let plans_dir = paths.root_dir.join("plans");
    let db_arc = Arc::new(db);
    let cfg = lsm_agent::agent::orchestrator::OrchestratorConfig {
        subagent_max_iterations: max_iterations,
        ..Default::default()
    };

    // -p 单轮模式每次生成独立 Session(debug 采集器以其 Session ID 命名归属)
    let mut session = Session::new();
    session
        .context_mut()
        .push(lsm_agent::llm::ChatMessage::user(prompt.clone()));

    let (orchestrator, collector) = if debug {
        let collector = Arc::new(lsm_agent::agent::debug::DebugCollector::new(session.id()));
        let decorated: Arc<dyn lsm_agent::llm::LlmClient> = Arc::new(
            lsm_agent::agent::debug::DebugLlmClient::new(llm.clone(), collector.clone()),
        );
        let cfg = lsm_agent::agent::orchestrator::OrchestratorConfig {
            debug: Some(collector.clone()),
            ..cfg
        };
        (
            MultiAgentOrchestrator::with_config(decorated, db_arc, plans_dir, cfg),
            Some(collector),
        )
    } else {
        (
            MultiAgentOrchestrator::with_config(llm.clone(), db_arc, plans_dir, cfg),
            None,
        )
    };

    // 取消传播:任务窗口监听 SIGINT,第一次中断取消当前任务(H9);
    // -p 单轮模式取消后按 128+SIGINT=130 惯例退出
    let cancel = lsm_agent::agent::cancel::CancelToken::new();
    let sig_cancel = cancel.clone();
    let sig_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        eprintln!("[laew] 收到中断信号,正在取消当前任务...");
        sig_cancel.cancel();
        let _ = tokio::signal::ctrl_c().await;
        std::process::exit(130);
    });
    let outcome = match orchestrator.handle_cancellable(&mut session, &cancel).await {
        Ok(o) => o,
        Err(e) if matches!(e, lsm_agent::error::AgentError::Cancelled) => {
            sig_task.abort();
            eprintln!("[laew] 任务已取消(用户中断)");
            std::process::exit(130);
        }
        Err(e) => {
            sig_task.abort();
            return Err(anyhow::Error::from(e));
        }
    };
    sig_task.abort();

    // debug 模式:任务结束后生成 Debug 报告(用未装饰的 llm 驱动 Debug Agent,避免自我采集递归)
    if let Some(collector) = collector {
        let meta = lsm_agent::agent::debug::ReportMeta {
            mode: mode.to_string(),
            task: prompt.clone(),
            model: format!(
                "[{}] {}/{} @ {}",
                active.protocol.as_str(),
                active.provider_name,
                active.model_name,
                active.end_point
            ),
        };
        let report_dir = paths.root_dir.join("DebugReport");
        match lsm_agent::agent::debug::finalize_report(&collector, llm.clone(), &report_dir, &meta)
            .await
        {
            Ok(path) => eprintln!("[laew] Debug 报告已生成: {}", path.display()),
            Err(e) => eprintln!("[laew] Debug 报告生成失败: {e}"),
        }
    }

    match outcome {
        OrchestrationOutcome::DirectAnswer { text, usage, .. } => {
            println!("{text}");
            print_usage(&usage);
        }
        OrchestrationOutcome::Executed { result } => {
            for wf in &result.workflows {
                println!("--- WorkFlow {} ({}) ---", wf.id, wf.name);
                println!("{}", wf.subflow_outcome);
            }
            if !result.summary.is_empty() {
                println!();
                println!("[session_context 摘要]");
                println!("{}", result.summary);
            }
            print_usage(&result.total_usage);
        }
        OrchestrationOutcome::Failed {
            suggestion, usage, ..
        } => {
            eprintln!("[agent failed] {suggestion}");
            print_usage(&usage);
        }
    }
    Ok(())
}

fn print_usage(usage: &lsm_agent::llm::Usage) {
    if usage.input_tokens > 0 || usage.output_tokens > 0 {
        eprintln!(
            "[laew] 用量: input={}  output={}{}{}",
            usage.input_tokens,
            usage.output_tokens,
            if usage.cache_read_input_tokens > 0 {
                format!("  cache_read={}", usage.cache_read_input_tokens)
            } else {
                String::new()
            },
            if usage.cache_creation_input_tokens > 0 {
                format!("  cache_creation={}", usage.cache_creation_input_tokens)
            } else {
                String::new()
            }
        );
    }
}

/// 从文件读取提示词并执行单轮任务
async fn run_from_file(file_path: PathBuf, max_iterations: usize, debug: bool) -> Result<()> {
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

    eprintln!(
        "[laew] 从文件读取提示词: {} ({} 字符)",
        absolute_path.display(),
        content.len()
    );
    run_one_shot(content, max_iterations, debug, "-f 文件").await
}

/// 解析文件路径：绝对路径直接使用，相对路径基于工作目录解析
fn resolve_path(file_path: PathBuf) -> Result<PathBuf> {
    if file_path.is_absolute() {
        Ok(file_path)
    } else {
        Ok(std::env::current_dir()?.join(file_path))
    }
}

/// 从 JSON 配置文件导入 Provider
async fn cmd_import_provider(file_path: PathBuf) -> Result<()> {
    let absolute_path = resolve_path(file_path)?;

    let content = std::fs::read_to_string(&absolute_path)
        .map_err(|e| anyhow::anyhow!("无法读取文件 '{}': {}", absolute_path.display(), e))?;

    let content = content.trim().to_string();
    if content.is_empty() {
        anyhow::bail!("文件 '{}' 内容为空", absolute_path.display());
    }

    let (_paths, db) = open_db()?;

    println!(
        "[laew] 正在从 '{}' 导入 Provider 配置...",
        absolute_path.display()
    );
    let result = db.import_from_json(&content).map_err(anyhow::Error::from)?;

    println!();
    println!("═══ 导入结果 ═══");
    println!("  成功: {}", result.success);
    println!("  跳过(重复): {}", result.skipped);
    println!("  失败: {}", result.failed);
    println!("  总计: {}", result.total());

    if result.failed > 0 {
        anyhow::bail!("部分记录导入失败，请检查上方日志");
    }

    Ok(())
}

/// 导出所有 Provider 到 JSON 配置文件
async fn cmd_export_provider(file_path: PathBuf) -> Result<()> {
    let absolute_path = resolve_path(file_path)?;

    // 确保父目录存在
    if let Some(parent) = absolute_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("无法创建目录 '{}': {}", parent.display(), e))?;
        }
    }

    let (_paths, db) = open_db()?;

    let json = db.export_to_json().map_err(anyhow::Error::from)?;

    std::fs::write(&absolute_path, &json)
        .map_err(|e| anyhow::anyhow!("无法写入文件 '{}': {}", absolute_path.display(), e))?;

    let parsed: serde_json::Value = serde_json::from_str(&json)?;
    let count = parsed["count"].as_u64().unwrap_or(0);

    println!(
        "[laew] 已导出 {} 条 Provider 记录到 '{}'",
        count,
        absolute_path.display()
    );

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // 崩溃取证必须先于 CLI 解析 / TUI 初始化 / Tokio worker 创建安装。
    // 报告目录沿用 laew 根目录约定，不依赖数据库配置，用户零配置。
    {
        let root_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        lsm_agent::crash::install_panic_hook_from_root(&root_dir);
    }

    let cli = {
        // 兼容用户习惯写法 `-debug` / `-inprovider` / `-outprovider`(单横线长参数),
        // 归一化为 `--xxx` 再交给 clap
        let args: Vec<std::ffi::OsString> = std::env::args_os()
            .map(|a| match a.to_str() {
                Some("-debug") => "--debug".into(),
                Some("-inprovider") => "--inprovider".into(),
                Some("-outprovider") => "--outprovider".into(),
                _ => a,
            })
            .collect();
        Cli::parse_from(args)
    };

    // TUI 模式下 INFO 级日志会与对话内容交错打印,造成视觉混乱 + 闪烁。
    // 单轮 / -debug / provider 子命令场景不受影响,沿用 RUST_LOG 默认行为。
    // 关联报告: 2026-09-09_07 F-007-1
    let is_tui = cli.prompt.is_none()
        && cli.file.is_none()
        && cli.inprovider.is_none()
        && cli.outprovider.is_none()
        && cli.cmd.is_none();
    let default_level = if is_tui { "warn" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_level.into()),
        )
        .with_target(false)
        .init();

    // 优先处理导入/导出命令
    if let Some(path) = cli.inprovider {
        cmd_import_provider(path).await
    } else if let Some(path) = cli.outprovider {
        cmd_export_provider(path).await
    } else {
        match cli.cmd {
            Some(Cmd::Provider(p)) => cmd_provider(p).await,
            None => {
                if let Some(prompt) = cli.prompt {
                    run_one_shot(prompt, cli.max_iterations, cli.debug, "-p 单轮").await
                } else if let Some(file_path) = cli.file {
                    run_from_file(file_path, cli.max_iterations, cli.debug).await
                } else {
                    lsm_agent::tui::run_with_debug(cli.debug).await
                }
            }
        }
    }
}
