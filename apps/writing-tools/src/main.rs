use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use writing_tools_core::commands::{find_command, run_command};
use writing_tools_core::config::AppConfig;
use writing_tools_core::providers::provider_from_config;

#[derive(Parser, Debug)]
#[command(name = "writing-tools", about = "Cross-platform Writing Tools (Rust core spike)")]
struct Cli {
    /// Override config path (default: ~/.config/writing-tools/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a default config file if missing, then print its path
    Init,
    /// List built-in / configured writing commands
    ListCommands,
    /// Run a writing command against text
    Run {
        /// Command id, e.g. proofread, rewrite, summary
        id: String,
        /// Input text. If omitted, read stdin.
        #[arg(short, long)]
        text: Option<String>,
        /// Extra instruction appended to the command prompt
        #[arg(long)]
        instruct: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let config_path = cli
        .config
        .unwrap_or_else(AppConfig::default_path);

    match cli.command {
        Commands::Init => {
            let cfg = AppConfig::load_or_init(&config_path)?;
            println!("config: {}", config_path.display());
            println!("provider: {:?} / {}", cfg.provider.kind, cfg.provider.model);
            println!("commands: {}", cfg.commands.len());
        }
        Commands::ListCommands => {
            let cfg = AppConfig::load_or_init(&config_path)?;
            for c in &cfg.commands {
                println!("{:<14} {:<12} {}", c.id, format!("{:?}", c.kind), c.label);
            }
        }
        Commands::Run { id, text, instruct } => {
            let cfg = AppConfig::load_or_init(&config_path)?;
            let input = match text {
                Some(t) => t,
                None => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .context("reading stdin")?;
                    buf
                }
            };
            let command = find_command(&cfg.commands, &id)?;
            let api_key = cfg.resolve_api_key()?;
            let provider = provider_from_config(
                cfg.provider.kind,
                &cfg.provider.base_url,
                &cfg.provider.model,
                &api_key,
            );
            let out = run_command(provider.as_ref(), command, &input, instruct.as_deref()).await?;
            println!("{out}");
        }
    }

    Ok(())
}
