use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use selara_core::commands::{
    find_command, run_command, run_command_stream, CommandKind, PromptVars,
};
use selara_core::config::{ApiKeySource, AppConfig};
use selara_core::secrets;

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
        /// Print the result only once it is complete. By default popup
        /// commands stream their output as the model writes it; replace
        /// commands always print the finished, cleaned text.
        #[arg(long)]
        no_stream: bool,
    },
    /// Start the desktop shell (global hotkey + picker UI). macOS only for now.
    Serve,
    /// Manage the provider API key in the OS keychain
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },
}

#[derive(Subcommand, Debug)]
enum KeyAction {
    /// Read a key from stdin and store it in the keychain for the configured
    /// provider; a key in config.toml is removed so the keychain copy wins.
    Set,
    /// Remove the keychain entry for the configured provider
    Clear,
    /// Show where the key would come from (env, keychain, config, none)
    Status,
}

fn describe_source(source: ApiKeySource) -> &'static str {
    match source {
        ApiKeySource::Env => "from environment (SELARA_API_KEY or WRITING_TOOLS_API_KEY)",
        ApiKeySource::Keychain => "from the OS keychain",
        ApiKeySource::Config => "from provider.api_key in config.toml (plaintext)",
        ApiKeySource::None => "not set",
    }
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
            println!("api key: {}", describe_source(cfg.api_key_source()));
        }
        Action::Key { action } => {
            let mut cfg = AppConfig::load_or_init(&config_path)?;
            let kind = cfg.provider.kind;
            match action {
                KeyAction::Set => {
                    use std::io::Read;
                    let mut raw = String::new();
                    std::io::stdin()
                        .read_to_string(&mut raw)
                        .context("reading API key from stdin")?;
                    secrets::keychain_set(kind, raw.trim())?;
                    if cfg.provider.api_key.is_some() {
                        cfg.provider.api_key = None;
                        cfg.save(&config_path)?;
                        println!("removed provider.api_key from {}", config_path.display());
                    }
                    println!(
                        "stored key for {:?} in the keychain ({} / {})",
                        kind,
                        secrets::KEYCHAIN_SERVICE,
                        secrets::keychain_account(kind)
                    );
                }
                KeyAction::Clear => {
                    secrets::keychain_delete(kind)?;
                    println!("removed keychain entry for {kind:?}");
                }
                KeyAction::Status => {
                    println!("{}", describe_source(cfg.api_key_source()));
                }
            }
        }
        Action::ListCommands => {
            let cfg = AppConfig::load_or_init(&config_path)?;
            for c in &cfg.commands {
                println!("{:<14} {:<12} {}", c.id, format!("{:?}", c.kind), c.label);
            }
        }
        Action::Run {
            id,
            text,
            instruct,
            no_stream,
        } => {
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
            let provider = cfg.build_provider_for(command)?;
            // Replace output is cleaned only once the reply is complete, so
            // it is never streamed; popup markdown is printed as it arrives.
            if no_stream || command.kind == CommandKind::Replace {
                let out = run_command(
                    provider.as_ref(),
                    command,
                    &input,
                    instruct.as_deref(),
                    Some(&cfg.language),
                )
                .await?;
                println!("{out}");
            } else {
                use std::io::Write;
                let mut stdout = std::io::stdout();
                let mut print_delta = |delta: &str| {
                    let _ = stdout.write_all(delta.as_bytes());
                    let _ = stdout.flush();
                };
                let result = run_command_stream(
                    provider.as_ref(),
                    command,
                    &input,
                    instruct.as_deref(),
                    PromptVars {
                        language: Some(&cfg.language),
                        app: None,
                    },
                    &mut print_delta,
                )
                .await;
                // End the streamed line even when the stream failed midway, so
                // the error on stderr does not continue a partial line.
                println!();
                result?;
            }
        }
        Action::Serve => unreachable!("handled in main"),
    }

    Ok(())
}
