use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use selara_core::commands::{find_command, run_command};
use selara_core::config::AppConfig;

#[cfg(target_os = "macos")]
mod serve;

#[derive(Parser, Debug)]
#[command(
    name = "selara",
    about = "Cross-platform Selara writing assistant (Rust)"
)]
struct Cli {
    /// Override config path (default: ~/.config/selara/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand, Debug)]
enum Action {
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
    /// Start the desktop shell (global hotkey + picker UI). macOS only for now.
    Serve,
}

fn main() -> Result<()> {
    // Default to info so `serve` still reports hotkey registration on stderr
    // when RUST_LOG is unset; RUST_LOG overrides as usual.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let config_path = cli.config.clone().unwrap_or_else(AppConfig::default_path);

    match cli.command {
        Action::Serve => {
            #[cfg(target_os = "macos")]
            {
                serve::run(config_path)?;
            }
            #[cfg(not(target_os = "macos"))]
            {
                anyhow::bail!("`serve` is currently only supported on macOS");
            }
        }
        other => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("tokio runtime")?;
            rt.block_on(async_cli(other, config_path))?;
        }
    }

    Ok(())
}

async fn async_cli(command: Action, config_path: PathBuf) -> Result<()> {
    match command {
        Action::Init => {
            let cfg = AppConfig::load_or_init(&config_path)?;
            println!("config: {}", config_path.display());
            println!(
                "provider: {:?} / {} (auth={:?})",
                cfg.provider.kind, cfg.provider.model, cfg.provider.auth
            );
            println!("hotkey: {}", cfg.hotkey);
            println!("commands: {}", cfg.commands.len());
        }
        Action::ListCommands => {
            let cfg = AppConfig::load_or_init(&config_path)?;
            for c in &cfg.commands {
                println!("{:<14} {:<12} {}", c.id, format!("{:?}", c.kind), c.label);
            }
        }
        Action::Run { id, text, instruct } => {
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
            let provider = cfg.build_provider()?;
            let out = run_command(
                provider.as_ref(),
                command,
                &input,
                instruct.as_deref(),
                Some(&cfg.language),
            )
            .await?;
            println!("{out}");
        }
        Action::Serve => unreachable!("handled in main"),
    }

    Ok(())
}
