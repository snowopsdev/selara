//! "Try it" for the command editor: run a draft prompt on sample text with the
//! active provider, exactly as a hotkey run would, without touching any app.

use selara_core::commands::{run_command_stream, CommandKind, PromptVars, WritingCommand};
use selara_core::config::AppConfig;
use selara_core::usage;
use serde::Serialize;
use std::path::Path;
use std::time::Instant;

/// What the editor shows under a successful try.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TryResult {
    /// The cleaned reply, as a real run would paste it.
    pub result: String,
    /// Wall-clock time of the whole run, provider start-up included.
    pub elapsed_ms: u64,
    /// Model the run used: the override when given, else the provider's.
    pub model: String,
    /// Usage ledger label of the provider (`UsageEvent::kind`).
    pub provider: String,
}

/// Reject what a real run would refuse, before any provider is built. The
/// sample is typed in Settings, so the secret guard does not apply, but the
/// hard size limit does: the try counts toward usage like any other run.
pub fn validate(prompt: &str, text: &str, hard_max_chars: u64) -> Result<(), String> {
    if prompt.trim().is_empty() {
        return Err("Write an instruction for the command first.".into());
    }
    if text.trim().is_empty() {
        return Err("Enter some sample text to try the command on.".into());
    }
    let chars = text.chars().count() as u64;
    if hard_max_chars > 0 && chars > hard_max_chars {
        return Err(format!(
            "Sample is {chars} characters, over your hard limit of {hard_max_chars}. \
             Shorten it or change Limits in Settings."
        ));
    }
    Ok(())
}

/// The draft as a transient Replace command. A blank model means "use the
/// provider's model", the same as an unset override on a saved command.
pub fn draft_command(prompt: &str, model: Option<&str>) -> WritingCommand {
    WritingCommand {
        id: "try".into(),
        label: "Try".into(),
        kind: CommandKind::Replace,
        prompt: prompt.to_string(),
        hotkey: None,
        model: model
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string),
        apps: Vec::new(),
        glyph: None,
        color: None,
        review: false,
    }
}

/// Run `prompt` on `text` through the same pipeline as `serve`: provider
/// built with the model override, streaming request, replacement cleanup,
/// and a usage ledger entry. Errors are the strings the UI shows. The sample
/// text is never logged.
pub async fn run(prompt: String, text: String, model: Option<String>) -> Result<TryResult, String> {
    run_at(&AppConfig::default_path(), prompt, text, model).await
}

/// [`run`] against the config at `config_path`; its usage ledger is the
/// `usage.jsonl` beside it.
pub async fn run_at(
    config_path: &Path,
    prompt: String,
    text: String,
    model: Option<String>,
) -> Result<TryResult, String> {
    let cfg = AppConfig::load_or_init(config_path).map_err(|e| e.to_string())?;
    validate(&prompt, &text, cfg.limits.hard_max_chars)?;
    let command = draft_command(&prompt, model.as_deref());
    // Settings does not record usage on its own; point the ledger at the
    // same file `serve` and the CLI append to before the provider runs.
    usage::set_store(Some(usage::usage_path(config_path)));
    let started = Instant::now();
    let provider = cfg
        .build_provider_for(&command)
        .map_err(|e| e.to_string())?;
    let result = run_command_stream(
        provider.as_ref(),
        &command,
        &text,
        None,
        PromptVars {
            language: Some(&cfg.language),
            app: None,
        },
        &mut |_: &str| {},
    )
    .await
    .map_err(|e| e.to_string())?;
    if result.trim().is_empty() {
        return Err("The provider returned no text.".into());
    }
    Ok(TryResult {
        result,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        model: cfg.effective_model(&command).to_string(),
        provider: cfg.usage_kind().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_blank_input_and_oversized_samples() {
        assert!(validate("  ", "text", 0)
            .unwrap_err()
            .contains("instruction"));
        assert!(validate("Fix", " \n ", 0)
            .unwrap_err()
            .contains("sample text"));
        let err = validate("Fix", "héllo", 4).unwrap_err();
        assert!(
            err.contains("5 characters") && err.contains("limit of 4"),
            "{err}"
        );
        assert!(
            validate("Fix", "héllo", 5).is_ok(),
            "the limit is inclusive"
        );
        assert!(
            validate("Fix", &"x".repeat(1_000), 0).is_ok(),
            "0 = no limit"
        );
    }

    #[test]
    fn draft_is_a_replace_command_with_an_optional_model() {
        let cmd = draft_command("Shorten it.", Some("  gpt-4.1-mini "));
        assert_eq!(cmd.kind, CommandKind::Replace);
        assert_eq!(cmd.prompt, "Shorten it.");
        assert_eq!(cmd.model.as_deref(), Some("gpt-4.1-mini"));
        assert_eq!(draft_command("x", Some("   ")).model, None);
        assert_eq!(draft_command("x", None).model, None);
        let cfg = AppConfig::default();
        assert_eq!(
            cfg.effective_model(&draft_command("x", None)),
            cfg.provider.model
        );
    }

    /// A fake Claude Code CLI that wraps its answer in quotes, the chat
    /// framing a real run strips before pasting.
    #[cfg(unix)]
    fn fake_cli(dir: &Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-claude");
        let reply = r#"{"type":"result","subtype":"success","is_error":false,"result":"\"Shorter.\"","usage":{"input_tokens":12,"output_tokens":3}}"#;
        std::fs::write(
            &path,
            format!("#!/bin/sh\ncat >/dev/null\nprintf '%s' '{reply}'\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn try_runs_the_real_pipeline_and_counts_toward_usage() {
        use selara_core::providers::ProviderKind;
        let dir = std::env::temp_dir().join(format!("selara-try-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        let binary = fake_cli(&dir);
        AppConfig::update(&config, |cfg| {
            cfg.provider.kind = ProviderKind::ClaudeCli;
            cfg.provider.cli_binary = Some(binary);
            cfg.provider.model = "provider-default".into();
            cfg.limits.hard_max_chars = 50;
            Ok(())
        })
        .unwrap();

        let out = run_at(
            &config,
            "Make it shorter.".into(),
            "This sentence could be a lot shorter.".into(),
            Some("try-test-model".into()),
        )
        .await
        .unwrap();
        assert_eq!(out.result, "Shorter.", "replacement cleanup applied");
        assert_eq!(out.model, "try-test-model");
        assert_eq!(out.provider, usage::KIND_CLAUDE_CLI);

        let events = usage::read_events(&usage::usage_path(&config)).unwrap();
        let recorded = events
            .iter()
            .find(|e| e.model == "try-test-model")
            .expect("the try is in the ledger");
        assert_eq!((recorded.input, recorded.output), (12, 3));
        assert!(recorded.duration_ms.is_some(), "the try is timed");

        let err = run_at(&config, "Fix".into(), "x".repeat(51), None)
            .await
            .unwrap_err();
        assert!(err.contains("hard limit of 50"), "{err}");
        usage::set_store(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn result_serializes_with_the_contract_field_names() {
        let value = serde_json::to_value(TryResult {
            result: "ok".into(),
            elapsed_ms: 12,
            model: "m".into(),
            provider: "anthropic".into(),
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"result": "ok", "elapsed_ms": 12, "model": "m", "provider": "anthropic"})
        );
    }
}
