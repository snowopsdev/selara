use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::chatgpt_auth::ChatGptAuth;
use crate::commands::{builtin_commands, WritingCommand};
use crate::error::CoreError;
use crate::providers::{provider_from_config, ChatGptCodexProvider, LlmProvider, ProviderKind};
use crate::secrets;

/// Where `resolve_api_key` would take the key from, in priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeySource {
    /// `SELARA_API_KEY` or the legacy `WRITING_TOOLS_API_KEY`.
    Env,
    /// OS credential store entry for the provider kind.
    Keychain,
    /// `provider.api_key` in `config.toml`.
    Config,
    /// Nothing found.
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuth {
    /// Bring-your-own-key (env or config.toml `api_key`). Default.
    #[default]
    ApiKey,
    /// Experimental: ChatGPT subscription via Codex CLI (`~/.codex/auth.json`).
    #[serde(rename = "chatgpt", alias = "chat_gpt")]
    ChatGpt,
}

/// Bump when a field changes meaning or a migration is needed. Files without
/// the key are treated as version 1 (everything written before it existed).
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

fn current_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Config file format version; see [`CURRENT_SCHEMA_VERSION`].
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,
    pub provider: ProviderConfig,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    /// Optional global shortcut that restores the text the last Replace
    /// overwrote. Unset means no shortcut (the picker still offers a button).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_hotkey: Option<String>,
    /// Preferred content/UI language code (e.g. "en", "es").
    #[serde(default = "default_language")]
    pub language: String,
    /// Omitted in TOML (README-style `[provider]`-only files) loads the built-ins.
    /// An explicit `commands = []` still means "no commands".
    #[serde(default = "builtin_commands")]
    pub commands: Vec<WritingCommand>,
    /// Selection / request size rails. Editable in the Settings UI; 0 disables a knob.
    #[serde(default)]
    pub limits: LimitsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    /// OpenAI-compatible base URL, e.g. https://api.openai.com/v1 or http://localhost:11434/v1
    pub base_url: String,
    pub model: String,
    /// Prefer env `SELARA_API_KEY` (or legacy `WRITING_TOOLS_API_KEY`) at runtime; this field is optional local storage.
    #[serde(default)]
    pub api_key: Option<String>,
    /// `api_key` (BYOK) or `chatgpt` (Experimental Codex CLI / ChatGPT subscription).
    /// Never stores ChatGPT tokens — those live in `~/.codex/auth.json`.
    #[serde(default)]
    pub auth: ProviderAuth,
}

/// Gentle defaults — accident protection, not rationing. Users with fat API budgets
/// can raise these or set a knob to `0` (unlimited) from Settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    /// Soft warn in the picker above this many characters. `0` = never warn.
    #[serde(default = "default_soft_warn_chars")]
    pub soft_warn_chars: u64,
    /// Hard refuse above this many characters. `0` = no hard limit.
    #[serde(default = "default_hard_max_chars")]
    pub hard_max_chars: u64,
    /// Extra caution before Replace above this size. `0` = never.
    #[serde(default = "default_replace_warn_chars")]
    pub replace_warn_chars: u64,
}

fn default_hotkey() -> String {
    "ctrl+shift+space".into()
}

fn default_language() -> String {
    "en".into()
}

fn default_soft_warn_chars() -> u64 {
    8_000
}

fn default_hard_max_chars() -> u64 {
    100_000
}

fn default_replace_warn_chars() -> u64 {
    4_000
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            soft_warn_chars: default_soft_warn_chars(),
            hard_max_chars: default_hard_max_chars(),
            replace_warn_chars: default_replace_warn_chars(),
        }
    }
}

/// One Settings tab's worth of config, for partial saves that leave the other
/// sections exactly as they are on disk.
#[derive(Debug, Clone, Deserialize)]
pub struct GeneralSection {
    pub hotkey: String,
    #[serde(default)]
    pub undo_hotkey: Option<String>,
    pub language: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            provider: ProviderConfig {
                kind: ProviderKind::OpenAiCompatible,
                base_url: "https://api.openai.com/v1".into(),
                model: "gpt-4o-mini".into(),
                api_key: None,
                auth: ProviderAuth::ApiKey,
            },
            hotkey: default_hotkey(),
            undo_hotkey: None,
            language: default_language(),
            commands: builtin_commands(),
            limits: LimitsConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn default_path() -> PathBuf {
        dirs_path().join("config.toml")
    }

    pub fn load_or_init(path: &Path) -> Result<Self, CoreError> {
        maybe_migrate_legacy_config(path)?;
        if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            let cfg: AppConfig = toml::from_str(&raw)?;
            // The file may hold an API key; older versions wrote it world-readable.
            restrict_to_owner(path);
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

    /// Write the config atomically: serialize to a sibling temp file created
    /// owner-only (0600), flush it, then rename it over `path`. A reader that
    /// polls the file (`selara serve`) sees either the old or the new content,
    /// never a truncated file, and the key never sits in a world-readable file.
    pub fn save(&self, path: &Path) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self).map_err(|e| CoreError::Config(e.to_string()))?;
        let tmp = temp_sibling(path);
        let _ = std::fs::remove_file(&tmp);
        write_private(&tmp, raw.as_bytes())?;
        if let Err(e) = replace_file(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Replace one section of the config from JSON and leave the rest untouched.
    /// `section` is `general`, `provider`, `commands`, or `limits`; `value` is the
    /// JSON the Settings UI holds for that tab. Used for partial saves so two
    /// writers (the Settings app and `serve`) do not clobber each other's fields.
    pub fn apply_section(
        &mut self,
        section: &str,
        value: serde_json::Value,
    ) -> Result<(), CoreError> {
        let bad = |e: serde_json::Error| CoreError::Config(format!("invalid `{section}`: {e}"));
        match section {
            "general" => {
                let g: GeneralSection = serde_json::from_value(value).map_err(bad)?;
                self.hotkey = if g.hotkey.trim().is_empty() {
                    default_hotkey()
                } else {
                    g.hotkey.trim().to_string()
                };
                self.undo_hotkey = g
                    .undo_hotkey
                    .map(|h| h.trim().to_string())
                    .filter(|h| !h.is_empty());
                self.language = if g.language.trim().is_empty() {
                    default_language()
                } else {
                    g.language.trim().to_string()
                };
            }
            "provider" => self.provider = serde_json::from_value(value).map_err(bad)?,
            "commands" => self.commands = serde_json::from_value(value).map_err(bad)?,
            "limits" => self.limits = serde_json::from_value(value).map_err(bad)?,
            other => {
                return Err(CoreError::Config(format!(
                    "unknown config section `{other}` (expected general, provider, commands, or limits)"
                )))
            }
        }
        Ok(())
    }

    /// Environment first, then the OS keychain entry for this provider kind,
    /// then `provider.api_key` in the file. A keychain that cannot be read
    /// (locked, denied) is logged and skipped so a key in the file still works.
    pub fn resolve_api_key(&self) -> Result<String, CoreError> {
        if let Some(key) = env_api_key() {
            return Ok(key);
        }
        match secrets::keychain_get(self.provider.kind) {
            Ok(Some(key)) => return Ok(key),
            Ok(None) => {}
            Err(e) => tracing::warn!("{e}; falling back to config.toml"),
        }
        self.provider
            .api_key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                CoreError::Config(
                    "missing API key: set SELARA_API_KEY, store one with `selara key set` \
(or the Settings app), or set provider.api_key in config.toml"
                        .into(),
                )
            })
    }

    /// Which source `resolve_api_key` would use right now.
    pub fn api_key_source(&self) -> ApiKeySource {
        if env_api_key().is_some() {
            return ApiKeySource::Env;
        }
        if matches!(secrets::keychain_get(self.provider.kind), Ok(Some(_))) {
            return ApiKeySource::Keychain;
        }
        if self
            .provider
            .api_key
            .as_deref()
            .is_some_and(|k| !k.trim().is_empty())
        {
            return ApiKeySource::Config;
        }
        ApiKeySource::None
    }

    /// Build the LLM provider for the current config.
    /// When `provider.auth = "chatgpt"` and kind is OpenAI-compatible, uses the
    /// experimental ChatGPT Codex backend (tokens from `~/.codex/auth.json`).
    pub fn build_provider(&self) -> Result<Box<dyn LlmProvider>, CoreError> {
        self.build_provider_with_model(&self.provider.model)
    }

    /// The model a command runs on: its own `model` override when set,
    /// otherwise `provider.model`.
    pub fn effective_model<'a>(&'a self, command: &'a WritingCommand) -> &'a str {
        command
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .unwrap_or(&self.provider.model)
    }

    /// [`build_provider`](Self::build_provider) honouring the command's model override.
    pub fn build_provider_for(
        &self,
        command: &WritingCommand,
    ) -> Result<Box<dyn LlmProvider>, CoreError> {
        self.build_provider_with_model(self.effective_model(command))
    }

    fn build_provider_with_model(&self, model: &str) -> Result<Box<dyn LlmProvider>, CoreError> {
        let use_chatgpt = matches!(self.provider.auth, ProviderAuth::ChatGpt)
            && matches!(self.provider.kind, ProviderKind::OpenAiCompatible);
        if use_chatgpt {
            let auth = ChatGptAuth::load()?;
            return Ok(Box::new(ChatGptCodexProvider::new(model.to_string(), auth)));
        }
        let api_key = self.resolve_api_key()?;
        Ok(provider_from_config(
            self.provider.kind,
            &self.provider.base_url,
            model,
            &api_key,
        ))
    }
}

fn env_api_key() -> Option<String> {
    ["SELARA_API_KEY", "WRITING_TOOLS_API_KEY"]
        .iter()
        .find_map(|var| std::env::var(var).ok())
        .filter(|k| !k.trim().is_empty())
}

fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".into());
    path.with_file_name(format!(".{name}.tmp-{}", std::process::id()))
}

/// Create (or truncate) `path` with owner-only permissions and write `data`.
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    Ok(())
}

fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // Windows will not rename over an existing file.
        let _ = std::fs::remove_file(to);
    }
    std::fs::rename(from, to)
}

/// Best-effort chmod 0600 for files that may contain an API key.
fn restrict_to_owner(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.permissions().mode() & 0o077 != 0 {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn dirs_path() -> PathBuf {
    // Avoid hard dependency on dirs crate in core; keep path logic local.
    // Prefer SELARA_CONFIG_DIR, then legacy WRITING_TOOLS_CONFIG_DIR, then ~/.config/selara.
    if let Some(base) = std::env::var_os("SELARA_CONFIG_DIR") {
        return PathBuf::from(base);
    }
    if let Some(base) = std::env::var_os("WRITING_TOOLS_CONFIG_DIR") {
        return PathBuf::from(base);
    }
    home_dir().join(".config").join("selara")
}

fn legacy_config_path() -> PathBuf {
    home_dir()
        .join(".config")
        .join("writing-tools")
        .join("config.toml")
}

/// If `path` is missing but a pre-rename Writing Tools config exists, copy it once.
fn maybe_migrate_legacy_config(path: &Path) -> Result<(), CoreError> {
    if path.exists() {
        return Ok(());
    }
    let legacy = legacy_config_path();
    if !legacy.exists() || legacy == path {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&legacy, path)?;
    eprintln!(
        "Migrated config from {} → {}",
        legacy.display(),
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_commands_load_builtins_explicit_empty_stays_empty() {
        let omitted = r#"
[provider]
kind = "open_ai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
"#;
        let cfg: AppConfig = toml::from_str(omitted).unwrap();
        assert_eq!(
            cfg.commands.len(),
            builtin_commands().len(),
            "README-style provider-only TOML must keep built-in commands"
        );
        assert!(cfg.commands.iter().any(|c| c.id == "proofread"));

        let empty = r#"
commands = []

[provider]
kind = "open_ai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
"#;
        let cfg: AppConfig = toml::from_str(empty).unwrap();
        assert!(
            cfg.commands.is_empty(),
            "explicit commands = [] must remain empty"
        );
    }

    #[test]
    fn ollama_kind_alias_loads_as_openai_compatible() {
        let raw = r#"
[provider]
kind = "ollama"
base_url = "http://localhost:11434/v1"
model = "llama3.1:8b"
"#;
        let cfg: AppConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.provider.kind, ProviderKind::OpenAiCompatible);
        assert_eq!(cfg.commands.len(), builtin_commands().len());
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("selara-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn schema_version_defaults_to_one_and_is_written() {
        let raw = r#"
[provider]
kind = "open_ai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
"#;
        let cfg: AppConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.schema_version, 1);
        let out = toml::to_string_pretty(&cfg).unwrap();
        assert!(out.contains("schema_version = 1"), "{out}");
    }

    #[test]
    fn save_is_atomic_and_owner_only() {
        let dir = scratch_dir("save");
        let path = dir.join("config.toml");
        AppConfig::default().save(&path).unwrap();
        // Overwrite once more so the rename-over-existing path runs too.
        let mut cfg = AppConfig::load_or_init(&path).unwrap();
        cfg.language = "fr".into();
        cfg.save(&path).unwrap();

        let entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec!["config.toml"],
            "no temp file may be left behind"
        );
        assert_eq!(AppConfig::load_or_init(&path).unwrap().language, "fr");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "config must be owner-only, got {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn load_tightens_permissions_of_existing_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch_dir("perm");
        let path = dir.join("config.toml");
        let raw = toml::to_string_pretty(&AppConfig::default()).unwrap();
        std::fs::write(&path, raw).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        AppConfig::load_or_init(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_section_replaces_only_that_section() {
        let mut cfg = AppConfig::default();
        cfg.limits.hard_max_chars = 42; // pretend `serve` changed this on disk

        cfg.apply_section(
            "general",
            serde_json::json!({"hotkey": " option+space ", "undo_hotkey": null, "language": ""}),
        )
        .unwrap();
        assert_eq!(cfg.hotkey, "option+space");
        assert_eq!(cfg.undo_hotkey, None);
        assert_eq!(cfg.language, "en", "blank language falls back to default");
        assert_eq!(cfg.limits.hard_max_chars, 42, "other sections untouched");

        cfg.apply_section(
            "general",
            serde_json::json!({"hotkey": "", "undo_hotkey": "ctrl+shift+z", "language": "es"}),
        )
        .unwrap();
        assert_eq!(cfg.hotkey, "ctrl+shift+space", "blank hotkey falls back");
        assert_eq!(cfg.undo_hotkey.as_deref(), Some("ctrl+shift+z"));

        cfg.apply_section(
            "provider",
            serde_json::json!({"kind": "anthropic", "base_url": "", "model": "claude-opus-5", "api_key": null, "auth": "api_key"}),
        )
        .unwrap();
        assert_eq!(cfg.provider.kind, ProviderKind::Anthropic);
        assert_eq!(cfg.provider.model, "claude-opus-5");

        cfg.apply_section(
            "commands",
            serde_json::json!([{"id": "x", "label": "X", "kind": "popup", "prompt": "Do X."}]),
        )
        .unwrap();
        assert_eq!(cfg.commands.len(), 1);
        assert_eq!(cfg.commands[0].id, "x");

        cfg.apply_section(
            "limits",
            serde_json::json!({"soft_warn_chars": 1, "hard_max_chars": 2, "replace_warn_chars": 3}),
        )
        .unwrap();
        assert_eq!(cfg.limits.hard_max_chars, 2);
        assert_eq!(cfg.limits.replace_warn_chars, 3);

        let err = cfg
            .apply_section("nope", serde_json::json!({}))
            .unwrap_err();
        assert!(err.to_string().contains("unknown config section"), "{err}");
        let err = cfg
            .apply_section("limits", serde_json::json!({"soft_warn_chars": "many"}))
            .unwrap_err();
        assert!(err.to_string().contains("invalid `limits`"), "{err}");
    }

    #[test]
    fn command_model_override_wins_over_provider_model() {
        let cfg = AppConfig::default();
        let mut cmd = cfg.commands[0].clone();
        assert_eq!(cfg.effective_model(&cmd), "gpt-4o-mini");
        cmd.model = Some("  gpt-4.1-mini ".into());
        assert_eq!(cfg.effective_model(&cmd), "gpt-4.1-mini");
        cmd.model = Some("   ".into());
        assert_eq!(
            cfg.effective_model(&cmd),
            "gpt-4o-mini",
            "blank override is ignored"
        );

        // Old TOML without the field loads; unset is not written back.
        let raw = toml::to_string_pretty(&cfg).unwrap();
        assert!(
            !raw.contains("model = \"gpt-4o-mini\"\n[[commands"),
            "{raw}"
        );
        let with = r#"
[provider]
kind = "open_ai_compatible"
base_url = ""
model = "gpt-4o-mini"

[[commands]]
id = "x"
label = "X"
kind = "replace"
prompt = "Do X."
model = "llama3.1:8b"
"#;
        let cfg: AppConfig = toml::from_str(with).unwrap();
        assert_eq!(cfg.commands[0].model.as_deref(), Some("llama3.1:8b"));
        assert_eq!(cfg.effective_model(&cfg.commands[0]), "llama3.1:8b");
    }

    #[test]
    fn undo_hotkey_is_optional_and_round_trips() {
        let without = r#"
[provider]
kind = "open_ai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
"#;
        let cfg: AppConfig = toml::from_str(without).unwrap();
        assert_eq!(cfg.undo_hotkey, None);
        let raw = toml::to_string_pretty(&cfg).unwrap();
        assert!(
            !raw.contains("undo_hotkey"),
            "unset undo hotkey must not be written: {raw}"
        );

        let with = format!("undo_hotkey = \"ctrl+shift+z\"\n{without}");
        let cfg: AppConfig = toml::from_str(&with).unwrap();
        assert_eq!(cfg.undo_hotkey.as_deref(), Some("ctrl+shift+z"));
        let raw = toml::to_string_pretty(&cfg).unwrap();
        let back: AppConfig = toml::from_str(&raw).unwrap();
        assert_eq!(back.undo_hotkey.as_deref(), Some("ctrl+shift+z"));
    }

    #[test]
    fn auth_defaults_to_api_key() {
        let raw = r#"
[provider]
kind = "open_ai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
"#;
        let cfg: AppConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.provider.auth, ProviderAuth::ApiKey);
    }

    #[test]
    fn auth_chatgpt_roundtrip() {
        let mut cfg = AppConfig::default();
        cfg.provider.auth = ProviderAuth::ChatGpt;
        cfg.provider.model = "gpt-5.4-mini".into();
        let raw = toml::to_string_pretty(&cfg).unwrap();
        assert!(
            raw.contains("auth = \"chatgpt\"") || raw.contains("auth = 'chatgpt'"),
            "expected auth = chatgpt in:\n{raw}"
        );
        let back: AppConfig = toml::from_str(&raw).unwrap();
        assert_eq!(back.provider.auth, ProviderAuth::ChatGpt);
        // UI spelling without rename would fail; alias also accepts chat_gpt
        let alt: AppConfig = toml::from_str(
            r#"
[provider]
kind = "open_ai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-5.4-mini"
auth = "chatgpt"
"#,
        )
        .unwrap();
        assert_eq!(alt.provider.auth, ProviderAuth::ChatGpt);
    }

    #[test]
    fn migrates_legacy_config_once() {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap();

        let tmp = std::env::temp_dir().join(format!("selara-migrate-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("home");
        let legacy_dir = home.join(".config").join("writing-tools");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy = legacy_dir.join("config.toml");
        let cfg = AppConfig {
            hotkey: "option+space".into(),
            ..Default::default()
        };
        cfg.save(&legacy).unwrap();

        let prev_home = std::env::var_os("HOME");
        let prev_selara = std::env::var_os("SELARA_CONFIG_DIR");
        let prev_wt = std::env::var_os("WRITING_TOOLS_CONFIG_DIR");
        std::env::set_var("HOME", &home);
        std::env::remove_var("SELARA_CONFIG_DIR");
        std::env::remove_var("WRITING_TOOLS_CONFIG_DIR");

        let dest = AppConfig::default_path();
        assert!(!dest.exists());
        let loaded = AppConfig::load_or_init(&dest).unwrap();
        assert!(
            dest.exists(),
            "expected migrated config at {}",
            dest.display()
        );
        assert_eq!(loaded.hotkey, "option+space");

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_selara {
            Some(v) => std::env::set_var("SELARA_CONFIG_DIR", v),
            None => std::env::remove_var("SELARA_CONFIG_DIR"),
        }
        match prev_wt {
            Some(v) => std::env::set_var("WRITING_TOOLS_CONFIG_DIR", v),
            None => std::env::remove_var("WRITING_TOOLS_CONFIG_DIR"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn keychain_sits_between_env_and_config() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        secrets::use_mock_store();
        let prev_selara = std::env::var_os("SELARA_API_KEY");
        let prev_wt = std::env::var_os("WRITING_TOOLS_API_KEY");
        std::env::remove_var("SELARA_API_KEY");
        std::env::remove_var("WRITING_TOOLS_API_KEY");

        let mut cfg = AppConfig::default();
        cfg.provider.kind = ProviderKind::Anthropic;
        secrets::keychain_delete(ProviderKind::Anthropic).unwrap();

        assert_eq!(cfg.api_key_source(), ApiKeySource::None);
        assert!(cfg.resolve_api_key().is_err());

        cfg.provider.api_key = Some("file-key".into());
        assert_eq!(cfg.api_key_source(), ApiKeySource::Config);
        assert_eq!(cfg.resolve_api_key().unwrap(), "file-key");

        secrets::keychain_set(ProviderKind::Anthropic, "chain-key").unwrap();
        assert_eq!(cfg.api_key_source(), ApiKeySource::Keychain);
        assert_eq!(cfg.resolve_api_key().unwrap(), "chain-key");

        std::env::set_var("SELARA_API_KEY", "env-key");
        assert_eq!(cfg.api_key_source(), ApiKeySource::Env);
        assert_eq!(cfg.resolve_api_key().unwrap(), "env-key");

        secrets::keychain_delete(ProviderKind::Anthropic).unwrap();
        match prev_selara {
            Some(v) => std::env::set_var("SELARA_API_KEY", v),
            None => std::env::remove_var("SELARA_API_KEY"),
        }
        match prev_wt {
            Some(v) => std::env::set_var("WRITING_TOOLS_API_KEY", v),
            None => std::env::remove_var("WRITING_TOOLS_API_KEY"),
        }
    }

    #[test]
    fn resolve_api_key_prefers_selara_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        secrets::use_mock_store();

        let prev_selara = std::env::var_os("SELARA_API_KEY");
        let prev_wt = std::env::var_os("WRITING_TOOLS_API_KEY");
        std::env::set_var("SELARA_API_KEY", "selara-key");
        std::env::set_var("WRITING_TOOLS_API_KEY", "legacy-key");
        let cfg = AppConfig::default();
        assert_eq!(cfg.resolve_api_key().unwrap(), "selara-key");
        std::env::remove_var("SELARA_API_KEY");
        assert_eq!(cfg.resolve_api_key().unwrap(), "legacy-key");
        match prev_selara {
            Some(v) => std::env::set_var("SELARA_API_KEY", v),
            None => std::env::remove_var("SELARA_API_KEY"),
        }
        match prev_wt {
            Some(v) => std::env::set_var("WRITING_TOOLS_API_KEY", v),
            None => std::env::remove_var("WRITING_TOOLS_API_KEY"),
        }
    }
}
