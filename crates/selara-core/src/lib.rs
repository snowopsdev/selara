//! Core writing pipeline: config, commands, and LLM providers.
//! No OS UI or accessibility APIs live here.

pub mod chatgpt_auth;
pub mod codex_cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod providers;

pub use chatgpt_auth::ChatGptAuth;
pub use codex_cli::CodexLoginStatus;
pub use commands::{
    build_system_prompt, builtin_commands, run_command, CommandKind, WritingCommand,
};
pub use config::{AppConfig, LimitsConfig, ProviderAuth, ProviderConfig};
pub use error::CoreError;
pub use providers::{
    list_chatgpt_models, list_provider_models, parse_sse_output_text_delta, provider_from_config,
    AnthropicProvider, ChatGptCodexProvider, CompletionRequest, LlmProvider,
    OpenAiCompatibleProvider, ProviderKind,
};
