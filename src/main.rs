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
    long_about = "LsmAgentEmergentWork (laew) 是由 LLM 驱动的 Rust Agent CLI。\n\
                  支持 Anthropic / OpenAI 双协议、多 Agent 协作(6 角色 + 三档难度)、\n\
                  TUI 多轮对话、单轮 -p / -f 文件模式。",
    after_help = "EXAMPLES:\n  \
                  # 跑一条单轮任务:\n    \
                  laew -p \"用一句话解释 Rust 所有权\"\n  \
                  \n  # 从文件读取提示词:\n    \
                  laew -f /path/to/prompt.md\n  \
                  \n  # 新增/列出/切换/删除 Provider:\n    \
                  laew provider add --protocol anthropic --provider-name anthropic --model-name claude-3-5-sonnet --end-point https://api.anthropic.com --api-key sk-ant-xxx\n    \
                  laew provider list\n    \
                  laew provider use 3\n    \
                  laew provider delete 5\n  \
                  \n  # 调试模式(任务结束后生成 Debug 报告):\n    \
                  laew -debug -p \"...\"\n  \
                  \n文档: docs/工程初始化方案/ 与 docs/TUI界面与CLI渲染引擎/",
    disable_help_subcommand = false
)]
struct Cli {
    /// 单轮任务提示词(不进入 TUI);与 -f 互斥
    #[arg(
        short = 'p',
        long = "prompt",
        value_name = "TEXT",
        conflicts_with = "file",
        help_heading = "输入来源"
    )]
    prompt: Option<String>,

    /// 从文件读取提示词(支持绝对/相对路径);与 -p 互斥
    #[arg(
        short = 'f',
        long = "file",
        value_name = "PATH",
        help_heading = "输入来源"
    )]
    file: Option<PathBuf>,

    /// 从 JSON 配置文件批量导入 Provider(支持单条/多条数组,也支持 -inprovider 单横线写法)
    #[arg(
        long = "inprovider",
        value_name = "PATH",
        conflicts_with_all = ["prompt", "file", "outprovider"],
        help_heading = "输入来源"
    )]
    inprovider: Option<PathBuf>,

    /// 导出所有 Provider 到 JSON 配置文件(也支持 -outprovider 单横线写法)
    #[arg(
        long = "outprovider",
        value_name = "PATH",
        conflicts_with_all = ["prompt", "file", "inprovider"],
        help_heading = "输入来源"
    )]
    outprovider: Option<PathBuf>,

    /// 最大 Agent 迭代次数(防止工具循环)
    #[arg(long, default_value_t = 16, global = true, help_heading = "运行调优")]
    max_iterations: usize,

    /// 调试模式:采集各 Agent 输入/输出/性能/质量,任务结束后由 Debug Agent 评估,
    /// 报告写入根目录 DebugReport/(也支持 `-debug` 单横线写法)
    #[arg(long = "debug", global = true, help_heading = "运行调优")]
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
#[command(after_help = "EXAMPLES:\n  \
                  # Anthropic 官方接入:\n    \
                  laew provider add --protocol anthropic --provider-name anthropic \\\n      \
                    --model-name claude-3-5-sonnet-20241022 \\\n      \
                    --end-point https://api.anthropic.com --api-key sk-ant-xxx\n  \
                  \n  # OpenAI 官方接入:\n    \
                  laew provider add --protocol openai --provider-name openai \\\n      \
                    --model-name gpt-4o --end-point https://api.openai.com --api-key sk-xxx\n  \
                  \n  # 自定义上下文上限(200K Token):\n    \
                  laew provider add --protocol anthropic --provider-name x \\\n      \
                    --model-name y --end-point https://example.com --api-key sk-z \\\n      \
                    --context-max-size 200K")]
enum ProviderCmd {
    /// 新增一条接入记录(若库为空则自动激活)
    Add {
        /// 协议类型: anthropic 或 openai
        #[arg(long, value_parser = parse_protocol)]
        protocol: Protocol,
        /// 自定义接入名(如 anthropic / openai / moonshot 等)
        #[arg(long)]
        provider_name: String,
        /// 模型名(如 claude-3-5-sonnet-20241022 / gpt-4o)
        #[arg(long)]
        model_name: String,
        /// 端点 URL(Anthropic 不要带 /v1/messages;OpenAI 不要带 /chat/completions;尾部 / 自动裁剪)
        #[arg(long)]
        end_point: String,
        /// API Key(可含 sk- 前缀;落库已脱敏,仅末尾 4 位可见)
        #[arg(long)]
        api_key: String,
        /// 上下文最大 Token 数(默认 800K;支持 800000/800K/1M 写法;0 = 不限制,关闭自动压缩)
        #[arg(long, value_parser = parse_context_size)]
        context_max_size: Option<u64>,
    },
    /// 列出全部接入记录(标记当前激活项)
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
    // -p/-f 单轮模式开启 CLI 等待心跳(TUI 模式不开,避免写坏 alternate screen)
    lsm_agent::llm::resilient::set_progress_feedback(true);
    let active = db
        .get_active_or_env()
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
    // D1 @ 提及展开(2026-09-10 第二十八轮,L1426):与 TUI dispatch_prompt 同一语义
    let expanded = lsm_agent::agent::attachments::expand_mentions(&prompt, &paths.work_dir);
    if expanded.attached > 0 {
        eprintln!("[laew] 已附加 {} 个 @ 提及内容", expanded.attached);
    }
    for m in &expanded.missed {
        eprintln!("[laew] 跳过 @ 提及: {m}");
    }
    let prompt = expanded.message;
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
    // 阶段进度(stderr 立即打印,stdout 保持只含答案与用量;与等待心跳同流,
    // 2026-09-10 第 23 轮 D05/D07 测试轮)
    let (stage_tx, mut stage_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let stage_printer = tokio::spawn(async move {
        while let Some(line) = stage_rx.recv().await {
            eprintln!("[stage] {line}");
        }
    });
    let outcome = match orchestrator
        .handle_cancellable_with_progress(&mut session, &cancel, Some(stage_tx))
        .await
    {
        Ok(o) => o,
        Err(e) if matches!(e, lsm_agent::error::AgentError::Cancelled) => {
            sig_task.abort();
            let _ = stage_printer.await;
            eprintln!("[laew] 任务已取消(用户中断)");
            std::process::exit(130);
        }
        Err(e) => {
            sig_task.abort();
            let _ = stage_printer.await;
            return Err(anyhow::Error::from(e));
        }
    };
    sig_task.abort();
    let _ = stage_printer.await;

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
            Ok(path) => eprintln!(
                "{}",
                lsm_agent::tui::pathfmt::fit_line(
                    "[laew] Debug 报告已生成: ",
                    &lsm_agent::tui::pathfmt::display_path(&paths, &path),
                    ""
                )
            ),
            Err(e) => eprintln!("[laew] Debug 报告生成失败: {e}"),
        }
    }

    match outcome {
        OrchestrationOutcome::DirectAnswer { text, usage, .. } => {
            println!("{}", lsm_agent::tui::sanitize_terminal_controls(&text));
            print_usage(&usage);
        }
        OrchestrationOutcome::Executed { result } => {
            // 2026-09-10 第二十九轮 P06/M05 自动化测试发现:
            // -p 单轮模式此前只打印 WorkFlow 文本,没有 [trace] 工具调用统计。
            // 与 TUI 模式 (src/tui/mod.rs:1229) 对齐,把每个 workflow 的 trace 行也补上,
            // 便于 -p 用户 / 脚本解析时区分 "LLM 直接 end_turn" vs "工具被调用"
            // vs "工具失败" 三种语义,改善可观测性。
            for wf in &result.workflows {
                println!(
                    "--- WorkFlow {} ({}) ---",
                    wf.id,
                    lsm_agent::tui::sanitize_terminal_controls(&wf.name)
                );
                println!(
                    "{}",
                    lsm_agent::tui::sanitize_terminal_controls(&wf.subflow_outcome)
                );
                if let Some(trace) = &wf.subflow_trace {
                    println!(
                        "[trace] iter={} tools={}(ok={},err={}) early_term={}",
                        trace.iterations,
                        trace.tool_calls,
                        trace.tool_calls_ok,
                        trace.tool_calls_err,
                        trace.early_terminated
                    );
                }
            }
            // 暴露 Yolo 三步分析(2026-09-10 第二十九轮 P06/M05):
            // 让用户在 -p 输出里能看到 Yolo 怎么理解任务,
            // 避免 mock Yolo 返回固定 JSON 时只看 mock 行为看不到 Yolo 决策。
            let c = &result.classification;
            let purpose = truncate_for_display(&c.purpose, 60);
            let goal = truncate_for_display(&c.goal_summary, 60);
            let intent = &c.intent;
            let plan_count = c.decomposition_plan.len();
            println!(
                "[yolo] purpose={} goal={} intent={} plan_steps={}",
                purpose, goal, intent, plan_count
            );
            if !result.summary.is_empty() {
                println!();
                println!("[session_context 摘要]");
                println!(
                    "{}",
                    lsm_agent::tui::sanitize_terminal_controls(&result.summary)
                );
            }
            print_usage(&result.total_usage);
        }
        OrchestrationOutcome::Failed {
            suggestion,
            reason,
            usage,
            ..
        } => {
            // F4(2026-09-10 第 25 轮):先呈现真实失败原因,再给建议
            if reason.is_empty() {
                eprintln!("[agent failed] {suggestion}");
            } else {
                eprintln!("[agent failed] 原因: {reason}");
                eprintln!("[agent failed] 建议: {suggestion}");
            }
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

/// Yolo 三步分析等文本字段在 stdout 里截断显示,防止脚本解析时一行过长。
/// CJK 字符按 1 个 char 计算(对应 1 列显示宽度);超过 limit 时末尾加 `…`。
fn truncate_for_display(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(limit.saturating_sub(1)).collect();
        out.push('…');
        out
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
    // SIGPIPE 必须早于任何 stdout 输出与 panic hook：`laew ... | head` 提前关闭
    // 管道是 Unix CLI 正常行为，不应进入 CrashDump 流程。
    lsm_agent::crash::restore_sigpipe_default();

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
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| default_level.into());
    if is_tui {
        // F5(2026-09-10 第 25 轮):TUI 模式 WARN 级 tracing 原始行直写 stdout,
        // 与全量重绘交错破坏对话区布局(D06 实测 3 段 6 行 WARN 裸奔在屏上)。
        // 改写入 {根目录}/logs/laew-tui.log(追加,目录/文件创建失败则静默丢弃,
        // 不影响主流程);关键降级事件已由 orchestrator 的 [stage] 进度行呈现。
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(false)
            .with_writer(lsm_agent::tui::tui_log_writer())
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(false)
            .with_writer(std::io::stderr)
            .init();
    }

    // 优先处理导入/导出命令
    if let Some(path) = cli.inprovider {
        cmd_import_provider(path).await
    } else if let Some(path) = cli.outprovider {
        cmd_export_provider(path).await
    } else {
        match cli.cmd {
            Some(Cmd::Provider(p)) => cmd_provider(p).await,
            None => {
                // 纯函数斜杠命令前置检测(/diff 等不依赖 LLM 的命令在 -p 模式也可直接执行)
                if let Some(prompt) = &cli.prompt {
                    if try_run_pure_slash_command(prompt) {
                        return Ok(());
                    }
                    run_one_shot(prompt.clone(), cli.max_iterations, cli.debug, "-p 单轮").await
                } else if let Some(file_path) = cli.file {
                    run_from_file(file_path, cli.max_iterations, cli.debug).await
                } else {
                    lsm_agent::tui::run_with_debug(cli.debug).await
                }
            }
        }
    }
}

/// 纯函数斜杠命令前置检测与执行。
///
/// 部分斜杠命令(如 `/diff`)不依赖 LLM,在 `-p` 单轮模式下也应直接执行,
/// 无需送入编排器(否则会被当作普通提示词处理)。
///
/// 返回 `true` 表示已处理完毕(调用者应直接 return Ok(()));
/// 返回 `false` 表示非纯函数命令,继续走常规 `-p` 编排流程。
fn try_run_pure_slash_command(input: &str) -> bool {
    let trimmed = input.trim();
    let Some(cmd) = trimmed.strip_prefix('/') else {
        return false;
    };

    // 取命令头(第一个空白符前的部分)
    let head = cmd.split_whitespace().next().unwrap_or("");
    let rest = cmd[head.len()..].trim();

    match head {
        "diff" => {
            // /diff <old_file> <new_file>:并排 diff 两个文件
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() < 2 {
                println!("  用法: /diff <旧文件路径> <新文件路径>");
                println!("  示例: /diff a.rs b.rs");
            } else {
                let old_path = parts[0];
                let new_path = parts[1];
                match lsm_agent::tui::render::diff::diff_files(old_path, new_path) {
                    Ok(hunk) => {
                        let rendered = lsm_agent::tui::render::diff::render_diff_hunk(&hunk);
                        for line_spans in rendered {
                            print!("  ");
                            for span in line_spans {
                                let attrs_ansi = tui_attrs_to_ansi(span.attrs);
                                print!(
                                    "\x1b[38;5;{}m{}{}\x1b[0m",
                                    tui_color_to_ansi256(span.fg),
                                    attrs_ansi,
                                    span.text
                                );
                            }
                            println!();
                        }
                    }
                    Err(e) => {
                        println!("  [diff 错误] 无法读取文件: {e}");
                    }
                }
            }
            true
        }
        _ => false,
    }
}

/// 把 `theme::attr` 位掩码转换为 ANSI 转义前缀(与 tui/mod.rs 内同名函数保持一致)。
fn tui_attrs_to_ansi(attrs: u8) -> String {
    let mut s = String::new();
    if attrs & lsm_agent::tui::theme::attr::BOLD != 0 {
        s.push_str("\x1b[1m");
    }
    if attrs & lsm_agent::tui::theme::attr::DIM != 0 {
        s.push_str("\x1b[2m");
    }
    if attrs & lsm_agent::tui::theme::attr::UNDERLINED != 0 {
        s.push_str("\x1b[4m");
    }
    if attrs & lsm_agent::tui::theme::attr::REVERSE != 0 {
        s.push_str("\x1b[7m");
    }
    s
}

/// 把 crossterm Color 转为 ANSI 256 色索引(与 tui/mod.rs 内同名函数保持一致)。
fn tui_color_to_ansi256(color: crossterm::style::Color) -> u8 {
    use crossterm::style::Color;
    match color {
        Color::Reset => 7,
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
