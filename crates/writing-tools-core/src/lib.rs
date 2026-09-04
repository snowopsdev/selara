//! Core writing pipeline: config, commands, and LLM providers.
//! No OS UI or accessibility APIs live here.

pub mod commands;
pub mod config;
pub mod error;
pub mod providers;

pub use commands::{builtin_commands, run_command, CommandKind, WritingCommand};
pub use config::{AppConfig, LimitsConfig, ProviderConfig};
pub use error::CoreError;
pub use providers::{CompletionRequest, LlmProvider, OpenAiCompatibleProvider, ProviderKind};
