use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use selara_core::commands::{find_command, run_command};
use selara_core::config::{ApiKeySource, AppConfig};
use selara_core::secrets;
use selara_core::usage;

#[cfg(target_os = "macos")]
mod progress;
#[cfg(target_os = "macos")]
mod serve;

#[derive(Parser, Debug)]
#[command(
    name = "selara",
    version,
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
        /// Retained for compatibility. Commands always print the complete,
        /// cleaned replacement text.
        #[arg(long)]
        no_stream: bool,
    },
    /// Start the desktop shell (command shortcuts + instruction dialog). macOS only for now.
    Serve {
        /// Use the Settings app's versioned JSON-lines control protocol.
        #[arg(long)]
        desktop_protocol: bool,
    },
    /// Show tokens used and estimated cost, from the local usage ledger
    Usage,
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
        Action::Serve { desktop_protocol } => {
            #[cfg(target_os = "macos")]
            {
                serve::run(config_path, desktop_protocol)?;
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = desktop_protocol;
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
    // Every command below may talk to a provider; record what it used.
    usage::set_store(Some(usage::usage_path(&config_path)));
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
            let cfg = AppConfig::load_or_init(&config_path)?;
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
                        AppConfig::update(&config_path, |latest| {
                            if latest.provider.kind == kind {
                                latest.provider.api_key = None;
                            }
                            Ok(())
                        })?;
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
            let _ = no_stream;
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

        Action::Usage => {
            let path = usage::usage_path(&config_path);
            let summary = usage::summary(&path)?;
            print!("{}", format_usage_table(&summary));
        }
        Action::Serve { .. } => unreachable!("handled in main"),
    }

    Ok(())
}

/// Render the ledger summary as a small fixed-width table.
fn format_usage_table(summary: &usage::UsageSummary) -> String {
    fn cost(bucket: &usage::UsageBucket) -> String {
        match bucket.cost_usd {
            Some(c) if bucket.unpriced > 0 => format!("~${c:.4} (+{} unpriced)", bucket.unpriced),
            Some(c) => format!("~${c:.4}"),
            None => "n/a".to_string(),
        }
    }
    let mut out = String::new();
    out.push_str(&format!("ledger: {}\n\n", summary.path));
    out.push_str(&format!(
        "{:<12} {:>9} {:>12} {:>12}  {}\n",
        "window", "requests", "tokens in", "tokens out", "est. cost"
    ));
    for (label, b) in [
        ("today", &summary.today),
        ("last 30 days", &summary.last_30_days),
        ("all time", &summary.all_time),
    ] {
        out.push_str(&format!(
            "{label:<12} {:>9} {:>12} {:>12}  {}\n",
            b.requests,
            b.input,
            b.output,
            cost(b)
        ));
    }
    if summary.models.is_empty() {
        out.push_str("\nno requests recorded yet\n");
    } else {
        out.push_str(&format!(
            "\n{:<18} {:<32} {:>9} {:>12} {:>12}  {}\n",
            "provider", "model", "requests", "tokens in", "tokens out", "est. cost"
        ));
        for m in &summary.models {
            out.push_str(&format!(
                "{:<18} {:<32} {:>9} {:>12} {:>12}  {}\n",
                m.kind,
                m.model,
                m.totals.requests,
                m.totals.input,
                m.totals.output,
                cost(&m.totals)
            ));
        }
    }
    out.push_str(
        "\ncosts are estimates from a built-in list-price table; local only, never sent anywhere\n",
    );
    out
}
