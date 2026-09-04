use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::commands::{builtin_commands, WritingCommand};
use crate::error::CoreError;
use crate::providers::ProviderKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub provider: ProviderConfig,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default)]
    pub commands: Vec<WritingCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    /// OpenAI-compatible base URL, e.g. https://api.openai.com/v1 or http://localhost:11434/v1
    pub base_url: String,
    pub model: String,
    /// Prefer env `WRITING_TOOLS_API_KEY` at runtime; this field is optional local storage.
    #[serde(default)]
    pub api_key: Option<String>,
}

fn default_hotkey() -> String {
    "ctrl+space".into()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            provider: ProviderConfig {
                kind: ProviderKind::OpenAiCompatible,
                base_url: "https://api.openai.com/v1".into(),
                model: "gpt-4o-mini".into(),
                api_key: None,
            },
            hotkey: default_hotkey(),
            commands: builtin_commands(),
        }
    }
}

impl AppConfig {
    pub fn default_path() -> PathBuf {
        dirs_path().join("config.toml")
    }

    pub fn load_or_init(path: &Path) -> Result<Self, CoreError> {
        if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            let cfg: AppConfig = toml::from_str(&raw)?;
            Ok(cfg)
        } else {
            let cfg = Self::default();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            cfg.save(path)?;
            Ok(cfg)
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self).map_err(|e| CoreError::Config(e.to_string()))?;
        std::fs::write(path, raw)?;
        Ok(())
    }

    pub fn resolve_api_key(&self) -> Result<String, CoreError> {
        if let Ok(key) = std::env::var("WRITING_TOOLS_API_KEY") {
            if !key.trim().is_empty() {
                return Ok(key);
            }
        }
        self.provider
            .api_key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                CoreError::Config(
                    "missing API key: set WRITING_TOOLS_API_KEY or provider.api_key in config.toml"
                        .into(),
                )
            })
    }
}

fn dirs_path() -> PathBuf {
    // Avoid hard dependency on dirs crate in core; keep path logic local.
    if let Some(base) = std::env::var_os("WRITING_TOOLS_CONFIG_DIR") {
        return PathBuf::from(base);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("writing-tools")
}
