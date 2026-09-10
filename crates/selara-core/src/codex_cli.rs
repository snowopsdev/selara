//! Account state from Codex's own account API; no credential-file parsing.
use crate::{app_server, error::CoreError};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct CodexLoginStatus {
    pub state: &'static str,
    pub logged_in: bool,
    pub message: String,
    pub via_chatgpt: bool,
    pub email: Option<String>,
    pub plan: Option<String>,
}
fn status(value: Value) -> CodexLoginStatus {
    let account = &value["account"];
    let kind = account["type"].as_str().unwrap_or("");
    let via_chatgpt = kind == "chatgpt";
    let api_key = kind == "apiKey";
    CodexLoginStatus {
        state: if via_chatgpt {
            "connected"
        } else if api_key {
            "api_key_only"
        } else {
            "signed_out"
        },
        logged_in: via_chatgpt || api_key,
        message: if via_chatgpt {
            "ChatGPT account connected"
        } else if api_key {
            "Codex is signed in with an API key. Sign in with ChatGPT to use subscription models."
        } else {
            "No ChatGPT account is signed in"
        }
        .into(),
        via_chatgpt,
        email: if via_chatgpt {
            account["email"].as_str().map(str::to_owned)
        } else {
            None
        },
        plan: if via_chatgpt {
            account["planType"].as_str().map(str::to_owned)
        } else {
            None
        },
    }
}
pub async fn login_status() -> Result<CodexLoginStatus, CoreError> {
    Ok(status(app_server::status().await?))
}
pub async fn cancel_login() -> Result<CodexLoginStatus, CoreError> {
    app_server::cancel_login().await?;
    login_status().await
}
pub async fn logout() -> Result<CodexLoginStatus, CoreError> {
    app_server::logout().await?;
    login_status().await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_subscription_key_and_missing_accounts() {
        let connected = status(
            serde_json::json!({"account":{"type":"chatgpt","email":"user@example.test","planType":"pro"}}),
        );
        assert!(connected.via_chatgpt);
        assert_eq!(connected.plan.as_deref(), Some("pro"));
        let no_email = status(serde_json::json!({"account":{"type":"chatgpt","email":null}}));
        assert!(no_email.logged_in && no_email.via_chatgpt);
        let key = status(serde_json::json!({"account":{"type":"apiKey"}}));
        assert!(key.logged_in);
        assert!(!key.via_chatgpt);
        assert_eq!(key.state, "api_key_only");
        assert_eq!(
            status(serde_json::json!({"account":null})).state,
            "signed_out"
        );
    }
}
