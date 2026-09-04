//! Shell out to the `codex` CLI for ChatGPT sign-in / status / logout.
//! Experimental — keeps tokens in Codex's auth store, not Writing Tools config.

use std::process::Command;

use serde::Serialize;

use crate::error::CoreError;

#[derive(Debug, Clone, Serialize)]
pub struct CodexLoginStatus {
    pub logged_in: bool,
    /// Raw stdout from `codex login status` (trimmed).
    pub message: String,
    /// True when status text indicates ChatGPT (vs API key) login.
    pub via_chatgpt: bool,
}

fn codex_bin() -> String {
    std::env::var("CODEX_BIN").unwrap_or_else(|_| "codex".into())
}

/// `codex login status` — exit 0 when logged in.
pub fn login_status() -> Result<CodexLoginStatus, CoreError> {
    let output = Command::new(codex_bin())
        .args(["login", "status"])
        .output()
        .map_err(|e| {
            CoreError::Config(format!(
                "failed to run `codex login status` (is the Codex CLI on PATH?): {e}"
            ))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let message = if !stdout.is_empty() {
        stdout
    } else {
        stderr
    };
    let logged_in = output.status.success();
    let via_chatgpt = message.to_lowercase().contains("chatgpt");
    Ok(CodexLoginStatus {
        logged_in,
        message,
        via_chatgpt,
    })
}

/// Interactive / browser ChatGPT login via `codex login`.
pub fn login() -> Result<CodexLoginStatus, CoreError> {
    let status = Command::new(codex_bin())
        .arg("login")
        .status()
        .map_err(|e| {
            CoreError::Config(format!(
                "failed to run `codex login` (is the Codex CLI on PATH?): {e}"
            ))
        })?;
    if !status.success() {
        return Err(CoreError::Config(format!(
            "`codex login` exited with {status}"
        )));
    }
    login_status()
}

/// `codex logout` — removes Codex-stored credentials.
pub fn logout() -> Result<CodexLoginStatus, CoreError> {
    let output = Command::new(codex_bin())
        .arg("logout")
        .output()
        .map_err(|e| {
            CoreError::Config(format!(
                "failed to run `codex logout` (is the Codex CLI on PATH?): {e}"
            ))
        })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        return Err(CoreError::Config(format!(
            "`codex logout` failed: {} {}",
            out.trim(),
            err.trim()
        )));
    }
    login_status()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_struct_serializes() {
        let s = CodexLoginStatus {
            logged_in: true,
            message: "Logged in using ChatGPT".into(),
            via_chatgpt: true,
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["logged_in"], true);
        assert_eq!(v["via_chatgpt"], true);
    }
}
