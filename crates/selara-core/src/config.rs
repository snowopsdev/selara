use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::commands::{builtin_commands, normalize_commands, WritingCommand};
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
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

fn default_schema_version() -> u32 {
    // A missing key is an old file. Keep that fact visible in memory until an
    // explicit write migrates it, so reading a config remains side-effect free.
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Config file format version; see [`CURRENT_SCHEMA_VERSION`].
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub provider: ProviderConfig,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
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
    /// Apps where the hotkeys do nothing (no window, no selection or clipboard
    /// read). Each entry is a localized app name (`1Password`), a bundle id
    /// (`com.apple.Terminal`), or a `prefix*` glob matching either; see
    /// [`app_is_excluded`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_apps: Vec<String>,
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
    /// Optional Codex home directory used by the app-server runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_home: Option<PathBuf>,
}

/// Gentle defaults — accident protection, not rationing. Users with fat API budgets
/// can raise these or set a knob to `0` (unlimited) from Settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    /// Soft warn before sending above this many characters. `0` = never warn.
    #[serde(default = "default_soft_warn_chars")]
    pub soft_warn_chars: u64,
    /// Hard refuse above this many characters. `0` = no hard limit.
    #[serde(default = "default_hard_max_chars")]
    pub hard_max_chars: u64,
    /// Extra caution before Replace above this size. `0` = never.
    #[serde(default = "default_replace_warn_chars")]
    pub replace_warn_chars: u64,
    /// Ask before sending text that looks like an API key, private key, JWT,
    /// or card number to a hosted provider. Local servers are exempt.
    #[serde(default = "default_true")]
    pub secret_guard: bool,
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

fn default_true() -> bool {
    true
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            soft_warn_chars: default_soft_warn_chars(),
            hard_max_chars: default_hard_max_chars(),
            replace_warn_chars: default_replace_warn_chars(),
            secret_guard: default_true(),
        }
    }
}

/// One Settings tab's worth of config, for partial saves that leave the other
/// sections exactly as they are on disk.
#[derive(Debug, Clone, Deserialize)]
pub struct GeneralSection {
    pub hotkey: String,
    pub language: String,
    /// Absent (older Settings builds) leaves the list on disk untouched.
    #[serde(default)]
    pub excluded_apps: Option<Vec<String>>,
}

/// Whether the frontmost app matches an `excluded_apps` entry, in which case
/// the hotkeys must not touch the selection.
///
/// Entries are trimmed and compared case-insensitively against both the
/// localized app name and the bundle id. An entry ending in `*` matches any
/// name or bundle id starting with the part before the `*`, so
/// `com.apple.*` covers every Apple app and `1Password*` covers
/// `1Password 8`. Blank entries never match.
pub fn app_is_excluded(
    excluded: &[String],
    app_name: Option<&str>,
    bundle_id: Option<&str>,
) -> bool {
    let name = app_name
        .map(|n| n.trim().to_lowercase())
        .filter(|n| !n.is_empty());
    let bundle = bundle_id
        .map(|b| b.trim().to_lowercase())
        .filter(|b| !b.is_empty());
    if name.is_none() && bundle.is_none() {
        return false;
    }
    let candidates = [name.as_deref(), bundle.as_deref()];
    excluded.iter().any(|entry| {
        let entry = entry.trim().to_lowercase();
        if entry.is_empty() || entry == "*" {
            return false;
        }
        match entry.strip_suffix('*') {
            Some(prefix) => candidates.iter().flatten().any(|c| c.starts_with(prefix)),
            None => candidates.iter().flatten().any(|c| *c == entry),
        }
    })
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
                codex_home: None,
            },
            hotkey: default_hotkey(),
            language: default_language(),
            commands: builtin_commands(),
            limits: LimitsConfig::default(),
            excluded_apps: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn default_path() -> PathBuf {
        dirs_path().join("config.toml")
    }

    /// Refuse a config written by a newer Selara. Deserializing drops fields
    /// this build does not know, and the next save would write the truncated
    /// struct back over the file while keeping the newer version number — so
    /// the newer release's settings would be silently deleted. Failing to
    /// start is recoverable; a clobbered config is not.
    fn check_schema_version(&self, path: &Path) -> Result<(), CoreError> {
        if self.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(CoreError::Config(format!(
                "{}: schema_version {} was written by a newer Selara (this build understands up to {}). \
                 Update Selara, or move that file aside to start from a fresh config.",
                path.display(),
                self.schema_version,
                CURRENT_SCHEMA_VERSION
            )));
        }
        Ok(())
    }

    pub fn load_or_init(path: &Path) -> Result<Self, CoreError> {
        if path.exists() {
            return load_existing(path);
        }

        // Initialization and legacy-file copying are writes, so serialize
        // them with the same cross-process lock used by update_config. The
        // second existence check handles another process winning the race.
        let _lock = ConfigLock::acquire(path)?;
        if path.exists() {
            return load_existing(path);
        }
        maybe_migrate_legacy_config(path)?;
        if path.exists() {
            return load_existing(path);
        }
        let cfg = Self::default();
        cfg.save_unlocked(path)?;
        Ok(cfg)
    }

    /// Write the config atomically: serialize to a sibling temp file created
    /// owner-only (0600), flush it, then rename it over `path`. A reader that
    /// polls the file (`selara serve`) sees either the old or the new content,
    /// never a truncated file, and the key never sits in a world-readable file.
    pub fn save(&self, path: &Path) -> Result<(), CoreError> {
        let _lock = ConfigLock::acquire(path)?;
        self.save_unlocked(path)
    }

    fn save_unlocked(&self, path: &Path) -> Result<(), CoreError> {
        self.check_schema_version(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Any explicit write is a migration boundary. Reads preserve the
        // source version in memory; writes emit the current schema and the
        // normalized command representation.
        let mut canonical = self.clone();
        canonical.schema_version = CURRENT_SCHEMA_VERSION;
        normalize_commands(&mut canonical.commands);
        let raw =
            toml::to_string_pretty(&canonical).map_err(|e| CoreError::Config(e.to_string()))?;
        let tmp = temp_sibling(path);
        let _ = std::fs::remove_file(&tmp);
        write_private(&tmp, raw.as_bytes())?;
        if let Err(e) = replace_file(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Re-read the latest config under a cross-process lock, apply a mutation,
    /// and persist one canonical schema-2 snapshot atomically.
    pub fn update<F>(path: &Path, update: F) -> Result<Self, CoreError>
    where
        F: FnOnce(&mut Self) -> Result<(), CoreError>,
    {
        update_config(path, update)
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
                self.language = if g.language.trim().is_empty() {
                    default_language()
                } else {
                    g.language.trim().to_string()
                };
                if let Some(apps) = g.excluded_apps {
                    self.excluded_apps = apps
                        .into_iter()
                        .map(|a| a.trim().to_string())
                        .filter(|a| !a.is_empty())
                        .collect();
                }
            }
            "provider" => self.provider = serde_json::from_value(value).map_err(bad)?,
            "commands" => {
                self.commands = serde_json::from_value(value).map_err(bad)?;
                normalize_commands(&mut self.commands);
            }
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
            let codex_home = crate::app_server::resolve_home(self.provider.codex_home.as_deref())?;
            return Ok(Box::new(ChatGptCodexProvider::new(
                model.to_string(),
                codex_home,
            )));
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

fn load_existing(path: &Path) -> Result<AppConfig, CoreError> {
    let raw = std::fs::read_to_string(path)?;
    let mut cfg: AppConfig = toml::from_str(&raw)?;
    // The file may hold an API key; older versions wrote it world-readable.
    restrict_to_owner(path);
    cfg.check_schema_version(path)?;
    // Legacy popup commands are still accepted by serde so old config files
    // remain readable, but the live command set has one replacement behavior.
    normalize_commands(&mut cfg.commands);
    Ok(cfg)
}

/// Re-read, mutate, and persist a config as one serialized read-modify-write.
///
/// The callback runs while a sibling lock file is held. Callers should keep it
/// focused on config state and avoid waiting on unrelated I/O; the resulting
/// snapshot is written atomically after it returns.
pub fn update_config<F>(path: &Path, update: F) -> Result<AppConfig, CoreError>
where
    F: FnOnce(&mut AppConfig) -> Result<(), CoreError>,
{
    let _lock = ConfigLock::acquire(path)?;
    maybe_migrate_legacy_config(path)?;
    let mut cfg = if path.exists() {
        load_existing(path)?
    } else {
        AppConfig::default()
    };
    update(&mut cfg)?;
    cfg.schema_version = CURRENT_SCHEMA_VERSION;
    normalize_commands(&mut cfg.commands);
    cfg.save_unlocked(path)?;
    Ok(cfg)
}

/// An advisory lock held by keeping the lock file open. The file itself is
/// persistent: unlinking it on drop would let a waiting process race with a
/// new opener and lock a different inode. The OS releases this lock when the
/// file descriptor closes, including when the process is killed.
struct ConfigLock {
    _file: std::fs::File,
}

impl ConfigLock {
    const WAIT: std::time::Duration = std::time::Duration::from_millis(10);
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

    fn acquire(config_path: &Path) -> Result<Self, CoreError> {
        let path = lock_path(config_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let started = std::time::Instant::now();
        loop {
            let mut opts = std::fs::OpenOptions::new();
            opts.read(true).write(true).create(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            match opts.open(&path) {
                Ok(file) => {
                    restrict_to_owner(&path);
                    match file.try_lock() {
                        Ok(()) => return Ok(Self { _file: file }),
                        Err(std::fs::TryLockError::WouldBlock) => {
                            if started.elapsed() >= Self::TIMEOUT {
                                return Err(CoreError::Config(format!(
                                    "timed out waiting for config lock {}",
                                    path.display()
                                )));
                            }
                            std::thread::sleep(Self::WAIT);
                        }
                        Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

fn lock_path(config_path: &Path) -> PathBuf {
    let name = config_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".into());
    config_path.with_file_name(format!(".{name}.lock"))
}

/// Path of the pidfile `selara serve` writes while it runs: `serve.pid` next
/// to the config file, so the Settings app (which knows the config path) can
/// tell whether the shell is running.
pub fn serve_pidfile(config_path: &Path) -> PathBuf {
    config_path.with_file_name("serve.pid")
}

fn env_api_key() -> Option<String> {
    // Filter inside the closure: an empty SELARA_API_KEY must not stop the
    // search before the legacy variable is examined.
    ["SELARA_API_KEY", "WRITING_TOOLS_API_KEY"]
        .iter()
        .find_map(|var| std::env::var(var).ok().filter(|k| !k.trim().is_empty()))
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

/// Best-effort chmod 0600 for files that may hold an API key or, in the case
/// of `history.jsonl`, verbatim selected text.
pub(crate) fn restrict_to_owner(path: &Path) {
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

    struct ChildGuard(std::process::Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if self.0.try_wait().unwrap().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    fn spawn_config_lock_child(path: &Path, mode: &str, field: &str, ready: &Path) -> ChildGuard {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "config::tests::config_lock_child_entrypoint",
                "--nocapture",
            ])
            .env("SELARA_CONFIG_LOCK_CHILD_MODE", mode)
            .env("SELARA_CONFIG_LOCK_CHILD_PATH", path)
            .env("SELARA_CONFIG_LOCK_CHILD_FIELD", field)
            .env("SELARA_CONFIG_LOCK_CHILD_READY", ready)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap();
        ChildGuard(child)
    }

    fn wait_for_child_ready(ready: &Path, children: &mut [ChildGuard]) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !ready.exists() {
            for child in children.iter_mut() {
                if let Some(status) = child.0.try_wait().unwrap() {
                    assert!(
                        status.success(),
                        "config lock child exited before acquiring lock: {status}"
                    );
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for config lock child"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Entry point used by the parent-process locking regression below. The
    /// environment guard keeps this test inert during the normal test run.
    #[test]
    fn config_lock_child_entrypoint() {
        let Some(mode) = std::env::var_os("SELARA_CONFIG_LOCK_CHILD_MODE") else {
            return;
        };
        let path = PathBuf::from(std::env::var_os("SELARA_CONFIG_LOCK_CHILD_PATH").unwrap());
        let ready = PathBuf::from(std::env::var_os("SELARA_CONFIG_LOCK_CHILD_READY").unwrap());
        match mode.to_string_lossy().as_ref() {
            "hold" => {
                let _lock = ConfigLock::acquire(&path).unwrap();
                std::fs::write(ready, b"ready").unwrap();
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
            "update" => {
                let field = std::env::var("SELARA_CONFIG_LOCK_CHILD_FIELD").unwrap();
                AppConfig::update(&path, |cfg| {
                    std::fs::write(&ready, b"ready").unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    match field.as_str() {
                        "language" => cfg.language = "child-language".into(),
                        "hotkey" => cfg.hotkey = "child-hotkey".into(),
                        other => panic!("unknown child field {other}"),
                    }
                    Ok(())
                })
                .unwrap();
            }
            other => panic!("unknown config lock child mode {other}"),
        }
    }

    #[test]
    fn config_lock_serializes_child_updates_and_recovers_after_kill() {
        let dir = scratch_dir("config-lock-processes");
        let path = dir.join("config.toml");
        AppConfig::default().save(&path).unwrap();

        let ready_language = dir.join("language.ready");
        let ready_hotkey = dir.join("hotkey.ready");
        let mut children = vec![
            spawn_config_lock_child(&path, "update", "language", &ready_language),
            spawn_config_lock_child(&path, "update", "hotkey", &ready_hotkey),
        ];
        wait_for_child_ready(&ready_language, &mut children);
        wait_for_child_ready(&ready_hotkey, &mut children);
        for child in &mut children {
            assert!(child.0.wait().unwrap().success());
        }

        let cfg = AppConfig::load_or_init(&path).unwrap();
        assert_eq!(cfg.language, "child-language");
        assert_eq!(cfg.hotkey, "child-hotkey");

        let ready_hold = dir.join("hold.ready");
        let mut holder = spawn_config_lock_child(&path, "hold", "", &ready_hold);
        wait_for_child_ready(&ready_hold, std::slice::from_mut(&mut holder));
        holder.0.kill().unwrap();
        let _ = holder.0.wait();

        let recovered = AppConfig::update(&path, |cfg| {
            cfg.language = "after-kill".into();
            Ok(())
        })
        .unwrap();
        assert_eq!(recovered.language, "after-kill");
        assert!(dir.join(".config.toml.lock").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serve_pidfile_is_sibling_of_config() {
        let cfg = Path::new("/home/me/.config/selara/config.toml");
        assert_eq!(
            serve_pidfile(cfg),
            PathBuf::from("/home/me/.config/selara/serve.pid")
        );
        // A custom --config path keeps the pidfile next to that file.
        let custom = Path::new("/tmp/work/my-selara.toml");
        assert_eq!(serve_pidfile(custom), PathBuf::from("/tmp/work/serve.pid"));
        let bare = Path::new("config.toml");
        assert_eq!(serve_pidfile(bare), PathBuf::from("serve.pid"));
    }

    #[test]
    fn versionless_config_defaults_to_one_and_explicit_save_migrates() {
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

        let dir = scratch_dir("schema-migrate");
        let path = dir.join("config.toml");
        std::fs::write(&path, raw).unwrap();
        let loaded = AppConfig::load_or_init(&path).unwrap();
        assert_eq!(loaded.schema_version, 1, "read normalization is in-memory");
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("schema_version"));
        loaded.save(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("schema_version = 2"), "{saved}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loading_legacy_popup_commands_normalizes_in_memory_without_rewriting() {
        let dir = scratch_dir("popup-migrate");
        let path = dir.join("config.toml");
        let raw = r#"
[provider]
kind = "open_ai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"

[[commands]]
id = "summary"
label = "Summary"
kind = "popup"
prompt = "Summarize in markdown."
"#;
        std::fs::write(&path, raw).unwrap();
        let cfg = AppConfig::load_or_init(&path).unwrap();
        assert_eq!(cfg.schema_version, 1);
        assert_eq!(cfg.commands[0].kind, crate::commands::CommandKind::Replace);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_config_rereads_under_lock_and_emits_schema_two() {
        let dir = scratch_dir("update-config");
        let path = dir.join("config.toml");
        let first = update_config(&path, |cfg| {
            cfg.language = "fr".into();
            Ok(())
        })
        .unwrap();
        assert_eq!(first.schema_version, CURRENT_SCHEMA_VERSION);
        let second = AppConfig::update(&path, |cfg| {
            assert_eq!(cfg.language, "fr");
            cfg.hotkey = "option+space".into();
            Ok(())
        })
        .unwrap();
        assert_eq!(second.hotkey, "option+space");
        assert_eq!(second.language, "fr");
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("schema_version = 2"), "{saved}");
        assert!(dir.join(".config.toml.lock").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_newer_schema_version_is_rejected_and_the_file_is_left_alone() {
        let dir = scratch_dir("newer-schema");
        let path = dir.join("config.toml");
        let raw = format!(
            r#"schema_version = {}
future_setting = "kept"

[provider]
kind = "open_ai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
"#,
            CURRENT_SCHEMA_VERSION + 1
        );
        std::fs::write(&path, &raw).unwrap();
        let err = AppConfig::load_or_init(&path).unwrap_err().to_string();
        assert!(
            err.contains(&(CURRENT_SCHEMA_VERSION + 1).to_string()) && err.contains("newer Selara"),
            "error must name the version it cannot read: {err}"
        );
        // Loading must not have rewritten the file, so the newer release's
        // fields are still there when it starts again.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_current_and_older_schema_versions_still_load() {
        let dir = scratch_dir("current-schema");
        let path = dir.join("config.toml");
        for version in [1, CURRENT_SCHEMA_VERSION] {
            let raw = format!(
                r#"schema_version = {version}

[provider]
kind = "open_ai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
"#
            );
            std::fs::write(&path, raw).unwrap();
            let cfg = AppConfig::load_or_init(&path).unwrap();
            assert_eq!(cfg.schema_version, version);
        }
        let _ = std::fs::remove_dir_all(&dir);
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

        let mut entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        assert_eq!(
            entries,
            vec![".config.toml.lock", "config.toml"],
            "only the persistent lock and config may remain"
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
        assert_eq!(cfg.language, "en", "blank language falls back to default");
        assert_eq!(cfg.limits.hard_max_chars, 42, "other sections untouched");

        cfg.apply_section(
            "general",
            serde_json::json!({"hotkey": "", "undo_hotkey": "ctrl+shift+z", "language": "es"}),
        )
        .unwrap();
        assert_eq!(cfg.hotkey, "ctrl+shift+space", "blank hotkey falls back");
        assert!(
            cfg.excluded_apps.is_empty(),
            "general save without excluded_apps leaves the list alone"
        );

        cfg.apply_section(
            "general",
            serde_json::json!({
                "hotkey": "", "undo_hotkey": null, "language": "",
                "excluded_apps": [" 1Password ", "", "com.apple.Terminal"]
            }),
        )
        .unwrap();
        assert_eq!(cfg.excluded_apps, vec!["1Password", "com.apple.Terminal"]);
        cfg.apply_section(
            "general",
            serde_json::json!({"hotkey": "", "undo_hotkey": null, "language": ""}),
        )
        .unwrap();
        assert_eq!(
            cfg.excluded_apps,
            vec!["1Password", "com.apple.Terminal"],
            "absent excluded_apps keeps the on-disk list"
        );

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
        assert!(cfg.limits.secret_guard, "omitted secret_guard stays on");

        cfg.apply_section(
            "limits",
            serde_json::json!({"soft_warn_chars": 1, "hard_max_chars": 2, "secret_guard": false}),
        )
        .unwrap();
        assert!(!cfg.limits.secret_guard);

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
    fn legacy_undo_hotkey_is_accepted_but_ignored() {
        let without = r#"
[provider]
kind = "open_ai_compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
"#;
        let cfg: AppConfig = toml::from_str(without).unwrap();
        let raw = toml::to_string_pretty(&cfg).unwrap();
        assert!(
            !raw.contains("undo_hotkey"),
            "unset undo hotkey must not be written: {raw}"
        );

        let with = format!("undo_hotkey = \"ctrl+shift+z\"\n{without}");
        let cfg: AppConfig = toml::from_str(&with).unwrap();
        let raw = toml::to_string_pretty(&cfg).unwrap();
        let back: AppConfig = toml::from_str(&raw).unwrap();
        assert_eq!(back.schema_version, 1);
        assert!(
            !raw.contains("undo_hotkey"),
            "legacy key must not be written: {raw}"
        );
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
        let _store = secrets::lock_mock_store();
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
    fn empty_preferred_env_falls_back_to_legacy_var() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _store = secrets::lock_mock_store();

        let prev_selara = std::env::var_os("SELARA_API_KEY");
        let prev_wt = std::env::var_os("WRITING_TOOLS_API_KEY");
        std::env::set_var("SELARA_API_KEY", "   ");
        std::env::set_var("WRITING_TOOLS_API_KEY", "legacy-key");
        let cfg = AppConfig::default();
        assert_eq!(cfg.api_key_source(), ApiKeySource::Env);
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

    #[test]
    fn resolve_api_key_prefers_selara_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _store = secrets::lock_mock_store();

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

    #[test]
    fn excluded_apps_round_trip_and_default() {
        let cfg = AppConfig::default();
        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(
            !toml_str.contains("excluded_apps"),
            "empty list is omitted from TOML"
        );
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.excluded_apps.is_empty());

        let with_apps: AppConfig = toml::from_str(
            "excluded_apps = [\"1Password\", \"com.apple.Terminal\"]\n\
             [provider]\nkind = \"open_ai_compatible\"\nbase_url = \"\"\nmodel = \"m\"\n",
        )
        .unwrap();
        assert_eq!(
            with_apps.excluded_apps,
            vec!["1Password", "com.apple.Terminal"]
        );
        let out = toml::to_string(&with_apps).unwrap();
        assert!(out.contains("excluded_apps = [\"1Password\", \"com.apple.Terminal\"]"));
    }

    #[test]
    fn app_is_excluded_matches_name_bundle_and_prefix() {
        let ex = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // Exact, case-insensitive name and bundle id.
        let list = ex(&["1Password", "com.apple.Terminal"]);
        assert!(app_is_excluded(
            &list,
            Some("1password"),
            Some("com.1password.1password")
        ));
        assert!(app_is_excluded(
            &list,
            Some("Terminal"),
            Some("COM.APPLE.TERMINAL")
        ));
        assert!(app_is_excluded(&list, None, Some("com.apple.terminal")));
        assert!(app_is_excluded(&list, Some("1Password"), None));
        assert!(!app_is_excluded(
            &list,
            Some("Safari"),
            Some("com.apple.Safari")
        ));
        assert!(!app_is_excluded(&list, None, None));

        // Prefix globs against either field.
        let list = ex(&["com.apple.*", "iTerm*"]);
        assert!(app_is_excluded(
            &list,
            Some("Safari"),
            Some("com.apple.Safari")
        ));
        assert!(app_is_excluded(
            &list,
            Some("iTerm2"),
            Some("com.googlecode.iterm2")
        ));
        assert!(!app_is_excluded(
            &list,
            Some("Ghostty"),
            Some("com.mitchellh.ghostty")
        ));

        // Whitespace, blanks, and a bare `*` never match everything.
        let list = ex(&["  Terminal  ", "", "   ", "*"]);
        assert!(app_is_excluded(&list, Some("Terminal"), None));
        assert!(!app_is_excluded(
            &list,
            Some("Notes"),
            Some("com.apple.Notes")
        ));

        // Empty list never excludes.
        assert!(!app_is_excluded(
            &[],
            Some("1Password"),
            Some("com.1password.1password")
        ));

        // Substrings without `*` do not match.
        let list = ex(&["Term"]);
        assert!(!app_is_excluded(
            &list,
            Some("Terminal"),
            Some("com.apple.Terminal")
        ));
    }
}
