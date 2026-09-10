//! Core writing pipeline: config, commands, and LLM providers.
//! No OS UI or accessibility APIs live here.

pub mod app_server;
pub mod codex_cli;
pub mod commands;
pub mod config;
pub mod desktop_protocol;
pub mod error;
pub mod guard;
pub mod history;
pub mod providers;
pub mod secrets;
pub mod usage;

pub use codex_cli::CodexLoginStatus;
pub use commands::{
    build_system_prompt, builtin_commands, run_command, CommandKind, WritingCommand,
};
pub use config::{AppConfig, LimitsConfig, ProviderAuth, ProviderConfig};
pub use error::CoreError;
pub use guard::{provider_is_hosted, scan_secrets, SecretHit, SecretKind};
pub use history::HistoryEntry;
pub use providers::{
    list_chatgpt_models, list_provider_models, parse_sse_output_text_delta, provider_from_config,
    take_complete_sse_events, AnthropicProvider, ChatGptCodexProvider, CompletionRequest,
    LlmProvider, OpenAiCompatibleProvider, ProviderKind,
};
pub use usage::{TokenUsage, UsageSummary};
