//! Load ChatGPT / Codex CLI tokens from `~/.codex/auth.json`.
//! Tokens are never written to Writing Tools config.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;

const OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// Public Codex CLI OAuth client id (same as `codex` CLI).
pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CODEX_CLI_VERSION: &str = "0.153.2";
pub const CODEX_USER_AGENT: &str = "codex_cli_rs/0.153.2";
pub const CODEX_ORIGINATOR: &str = "codex_cli_rs";
pub const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const CODEX_MODELS_URL: &str =
    "https://chatgpt.com/backend-api/codex/models?client_version=0.153.2";

/// Approximate access-token lifetime before we proactively refresh (ChatGPT tokens are short-lived).
const REFRESH_SKEW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatGptAuthFile {
    #[serde(default)]
    pub auth_mode: Option<String>,
    pub tokens: ChatGptTokens,
    #[serde(default)]
    pub last_refresh: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatGptTokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChatGptAuth {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: Option<String>,
    pub account_id: Option<String>,
    pub last_refresh: Option<String>,
    pub path: PathBuf,
}

impl ChatGptAuth {
    pub fn auth_json_path() -> PathBuf {
        if let Some(p) = std::env::var_os("CODEX_AUTH_JSON") {
            return PathBuf::from(p);
        }
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(".codex").join("auth.json")
    }

    pub fn load() -> Result<Self, CoreError> {
        Self::load_from(&Self::auth_json_path())
    }

    pub fn load_from(path: &Path) -> Result<Self, CoreError> {
        if !path.exists() {
            return Err(CoreError::Config(format!(
                "ChatGPT auth not found at {}. Run `codex login` or use Settings → Sign in with ChatGPT (Experimental).",
                path.display()
            )));
        }
        let raw = fs::read_to_string(path)?;
        let file: ChatGptAuthFile = serde_json::from_str(&raw).map_err(|e| {
            CoreError::Config(format!("invalid {}: {e}", path.display()))
        })?;
        if file.tokens.access_token.trim().is_empty() {
            return Err(CoreError::Config(
                "ChatGPT auth.json has empty access_token".into(),
            ));
        }
        Ok(Self {
            access_token: file.tokens.access_token,
            refresh_token: file.tokens.refresh_token,
            id_token: file.tokens.id_token,
            account_id: file.tokens.account_id,
            last_refresh: file.last_refresh,
            path: path.to_path_buf(),
        })
    }

    pub fn account_id_header(&self) -> Option<&str> {
        self.account_id.as_deref().filter(|s| !s.trim().is_empty())
    }

    /// Refresh tokens via OpenAI OAuth and write back to auth.json (mode 0600).
    pub async fn refresh(&mut self) -> Result<(), CoreError> {
        let client = reqwest::Client::new();
        let body = json!({
            "client_id": CODEX_OAUTH_CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": self.refresh_token,
        });
        let resp = client
            .post(OAUTH_TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let value: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            return Err(CoreError::Provider(format!(
                "ChatGPT token refresh failed HTTP {status}: {value}"
            )));
        }
        let access = value
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::Provider(format!("refresh missing access_token: {value}")))?
            .to_string();
        let refresh = value
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.refresh_token)
            .to_string();
        let id_token = value
            .get("id_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| self.id_token.clone());

        self.access_token = access;
        self.refresh_token = refresh;
        self.id_token = id_token;
        self.last_refresh = Some(chrono_like_now());
        self.write_back()?;
        Ok(())
    }

    pub fn write_back(&self) -> Result<(), CoreError> {
        let file = ChatGptAuthFile {
            auth_mode: Some("chatgpt".into()),
            tokens: ChatGptTokens {
                access_token: self.access_token.clone(),
                refresh_token: self.refresh_token.clone(),
                id_token: self.id_token.clone(),
                account_id: self.account_id.clone(),
            },
            last_refresh: self.last_refresh.clone(),
        };
        let raw = serde_json::to_string_pretty(&file)
            .map_err(|e| CoreError::Config(e.to_string()))?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, raw)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// Best-effort: refresh if JWT `exp` is missing/near, or if load looks stale.
    pub async fn ensure_fresh(&mut self) -> Result<(), CoreError> {
        if access_token_needs_refresh(&self.access_token) {
            self.refresh().await?;
        }
        Ok(())
    }
}

fn chrono_like_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // ISO-ish without pulling chrono; Codex accepts any string here.
    format!("{secs}")
}

/// Decode JWT payload (no verify) and check `exp`.
pub fn access_token_needs_refresh(access_token: &str) -> bool {
    let Some(exp) = jwt_exp(access_token) else {
        // Unknown shape — try refresh once so a bad token surfaces clearly.
        return true;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now + REFRESH_SKEW.as_secs() >= exp
}

fn jwt_exp(token: &str) -> Option<u64> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    let json_bytes = b64url_decode(payload_b64)?;
    let value: serde_json::Value = serde_json::from_slice(&json_bytes).ok()?;
    value.get("exp")?.as_u64()
}

fn b64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut s = input.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    base64_decode(&s)
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    // Minimal base64 decoder for JWT payloads (no extra crate).
    const TABLE: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: i32 = 0;
    for &c in input.as_bytes() {
        if c == b'=' {
            break;
        }
        let val = TABLE.iter().position(|&x| x == c)? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fake_jwt(exp: u64) -> String {
        // header.payload.sig — payload is {"sub":"test","exp":N}
        let header = "eyJhbGciOiJub25lIn0"; // {"alg":"none"}
        let payload = format!(r#"{{"sub":"test","exp":{exp}}}"#);
        let payload_b64 = b64url_encode(payload.as_bytes());
        format!("{header}.{payload_b64}.sig")
    }

    fn b64url_encode(data: &[u8]) -> String {
        const TABLE: &[u8] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let mut n = 0u32;
            for (i, &b) in chunk.iter().enumerate() {
                n |= (b as u32) << (16 - 8 * i);
            }
            let pads = 3 - chunk.len();
            let chars = match pads {
                0 => 4,
                1 => 3,
                _ => 2,
            };
            for i in 0..chars {
                let idx = ((n >> (18 - 6 * i)) & 0x3f) as usize;
                out.push(TABLE[idx] as char);
            }
        }
        out.replace('+', "-").replace('/', "_")
    }

    #[test]
    fn jwt_exp_and_refresh_skew() {
        let far = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let token = fake_jwt(far);
        assert!(!access_token_needs_refresh(&token));

        let past = 1_700_000_000u64;
        let expired = fake_jwt(past);
        assert!(access_token_needs_refresh(&expired));
    }

    #[test]
    fn load_fixture_auth_json() {
        let dir = tempfile_dir();
        let path = dir.join("auth.json");
        let token = fake_jwt(4_000_000_000);
        let body = json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": token,
                "refresh_token": "rt-fixture",
                "id_token": "id-fixture",
                "account_id": "acct-fixture"
            },
            "last_refresh": "2026-01-01T00:00:00Z"
        });
        let mut f = fs::File::create(&path).unwrap();
        write!(f, "{}", body).unwrap();
        let auth = ChatGptAuth::load_from(&path).unwrap();
        assert_eq!(auth.account_id.as_deref(), Some("acct-fixture"));
        assert_eq!(auth.refresh_token, "rt-fixture");
        assert!(!access_token_needs_refresh(&auth.access_token));
    }

    fn tempfile_dir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("wt-chatgpt-auth-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }
}
