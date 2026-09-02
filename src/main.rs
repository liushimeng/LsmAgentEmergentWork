use std::sync::Arc;

use clap::Parser;
use lsm_agent::agent::Agent;
use lsm_agent::llm::OpenAiClient;
use lsm_agent::tool::builtin_registry;

/// LsmAgentEmergentWork - 命令行 Agent
#[derive(Parser)]
#[command(name = "lsm-agent", version, about)]
struct Cli {
    /// 任务/问题
    task: String,

    /// OpenAI 兼容接口地址, 也可用环境变量 LLM_BASE_URL
    #[arg(long, env = "LLM_BASE_URL", default_value = "https://api.openai.com")]
    base_url: String,

    /// API Key, 也可用环境变量 LLM_API_KEY
    #[arg(long, env = "LLM_API_KEY")]
    api_key: String,

    /// 模型名, 也可用环境变量 LLM_MODEL
    #[arg(long, env = "LLM_MODEL", default_value = "gpt-4o-mini")]
    model: String,

    /// 最大迭代次数
    #[arg(long, default_value_t = 10)]
    max_iterations: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    let llm = Arc::new(OpenAiClient::new(cli.base_url, cli.api_key, cli.model));
    let agent = Agent::new(
        llm,
        builtin_registry(),
        "你是一个乐于助人的 Agent, 可以使用提供的工具完成任务。",
    )
    .with_max_iterations(cli.max_iterations);

    let answer = agent.run(&cli.task).await?;
    println!("{answer}");
    Ok(())
}
