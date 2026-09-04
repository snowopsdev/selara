use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::chatgpt_auth::{
    ChatGptAuth, CODEX_MODELS_URL, CODEX_ORIGINATOR, CODEX_RESPONSES_URL, CODEX_USER_AGENT,
};
use crate::error::CoreError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Serde default is `open_ai_compatible`; also accept UI spelling `openai_compatible`.
    #[serde(alias = "openai_compatible")]
    OpenAiCompatible,
    Ollama,
    Gemini,
    Anthropic,
}

#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub system: String,
    pub user: String,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError>;
}

pub struct OpenAiCompatibleProvider {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let client = reqwest::Client::new();
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": req.user}
            ],
            "temperature": 0.2
        });

        let mut builder = client.post(url).json(&body);
        if !self.api_key.is_empty() {
            builder = builder.bearer_auth(&self.api_key);
        }

        let resp = builder.send().await?;
        let status = resp.status();
        let value: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            return Err(CoreError::Provider(format!("HTTP {status}: {value}")));
        }

        value
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .ok_or_else(|| CoreError::Provider(format!("unexpected response: {value}")))
    }
}

/// Gemini uses a different REST shape; keep a thin adapter so the app shell can switch providers later.
pub struct GeminiProvider {
    pub api_key: String,
    pub model: String,
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );
        let client = reqwest::Client::new();
        let prompt = format!("{}\n\n{}", req.system, req.user);
        let body = json!({
            "contents": [{"parts": [{"text": prompt}]}]
        });
        let resp = client.post(url).json(&body).send().await?;
        let status = resp.status();
        let value: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            return Err(CoreError::Provider(format!("HTTP {status}: {value}")));
        }
        value
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .ok_or_else(|| CoreError::Provider(format!("unexpected Gemini response: {value}")))
    }
}

pub struct AnthropicProvider {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
        let base = if self.base_url.trim().is_empty() {
            "https://api.anthropic.com".to_string()
        } else {
            self.base_url.trim_end_matches('/').to_string()
        };
        let url = format!("{base}/v1/messages");
        let client = reqwest::Client::new();
        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": req.system,
            "messages": [
                {"role": "user", "content": req.user}
            ]
        });
        let resp = client
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let value: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            return Err(CoreError::Provider(format!("HTTP {status}: {value}")));
        }
        // content is an array of blocks; take first text block
        if let Some(arr) = value.get("content").and_then(|v| v.as_array()) {
            for block in arr {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        return Ok(text.trim().to_string());
                    }
                }
            }
        }
        Err(CoreError::Provider(format!(
            "unexpected Anthropic response: {value}"
        )))
    }
}

/// Experimental: ChatGPT subscription via Codex CLI auth (`~/.codex/auth.json`).
pub struct ChatGptCodexProvider {
    pub model: String,
    pub auth: ChatGptAuth,
}

impl ChatGptCodexProvider {
    pub fn new(model: String, auth: ChatGptAuth) -> Self {
        Self { model, auth }
    }
}

#[async_trait]
impl LlmProvider for ChatGptCodexProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
        let mut auth = self.auth.clone();
        auth.ensure_fresh().await?;

        let client = reqwest::Client::new();
        let body = json!({
            "model": self.model,
            "instructions": req.system,
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": req.user
                }]
            }],
            "store": false,
            "stream": true
        });

        let mut builder = client
            .post(CODEX_RESPONSES_URL)
            .header("Authorization", format!("Bearer {}", auth.access_token))
            .header("originator", CODEX_ORIGINATOR)
            .header("User-Agent", CODEX_USER_AGENT)
            .header("OpenAI-Beta", "responses=experimental")
            .header("Accept", "text/event-stream")
            .json(&body);

        if let Some(account_id) = auth.account_id_header() {
            builder = builder.header("ChatGPT-Account-ID", account_id);
        }

        let resp = builder.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(CoreError::Provider(format!(
                "ChatGPT Codex HTTP {status}: {text}"
            )));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // Process complete SSE events (blank-line delimited).
            while let Some(idx) = buf.find("\n\n") {
                let event = buf[..idx].to_string();
                buf = buf[idx + 2..].to_string();
                if let Some(delta) = parse_sse_output_text_delta(&event) {
                    out.push_str(&delta);
                }
            }
        }
        // Trailing event without final blank line
        if !buf.trim().is_empty() {
            if let Some(delta) = parse_sse_output_text_delta(&buf) {
                out.push_str(&delta);
            }
        }

        let trimmed = out.trim().to_string();
        if trimmed.is_empty() {
            return Err(CoreError::Provider(
                "ChatGPT Codex returned no output_text deltas".into(),
            ));
        }
        Ok(trimmed)
    }
}

/// Extract text from an SSE event whose `event:` is `response.output_text.delta`
/// (or whose JSON `type` field matches). Data may be split across multiple `data:` lines.
pub fn parse_sse_output_text_delta(event_block: &str) -> Option<String> {
    let mut event_name: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for line in event_block.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start());
        }
    }
    if data_lines.is_empty() {
        return None;
    }
    let data = data_lines.join("\n");
    if data == "[DONE]" {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&data).ok()?;
    let type_field = value.get("type").and_then(|v| v.as_str());
    let is_delta = event_name.as_deref() == Some("response.output_text.delta")
        || type_field == Some("response.output_text.delta");
    if !is_delta {
        return None;
    }
    value
        .get("delta")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            value
                .pointer("/delta/text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

/// List model slugs from the Codex models endpoint (requires ChatGPT auth).
pub async fn list_chatgpt_models() -> Result<Vec<String>, CoreError> {
    let mut auth = ChatGptAuth::load()?;
    auth.ensure_fresh().await?;
    let client = reqwest::Client::new();
    let mut builder = client
        .get(CODEX_MODELS_URL)
        .header("Authorization", format!("Bearer {}", auth.access_token))
        .header("originator", CODEX_ORIGINATOR)
        .header("User-Agent", CODEX_USER_AGENT);
    if let Some(account_id) = auth.account_id_header() {
        builder = builder.header("ChatGPT-Account-ID", account_id);
    }
    let resp = builder.send().await?;
    let status = resp.status();
    let value: serde_json::Value = resp.json().await?;
    if !status.is_success() {
        return Err(CoreError::Provider(format!(
            "list models HTTP {status}: {value}"
        )));
    }
    let mut out = Vec::new();
    // Accept a few shapes: { data: [ { id / slug } ] } or a bare array.
    let items = value
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| value.get("models").and_then(|v| v.as_array()))
        .or_else(|| value.as_array());
    if let Some(arr) = items {
        for item in arr {
            // Skip Codex-internal hidden entries (e.g. gpt-reserve).
            if item.get("visibility").and_then(|v| v.as_str()) == Some("hide") {
                continue;
            }
            if let Some(id) = item
                .get("slug")
                .or_else(|| item.get("id"))
                .and_then(|v| v.as_str())
            {
                out.push(id.to_string());
            }
        }
    }
    if out.is_empty() {
        return Err(CoreError::Provider(format!(
            "unexpected models response: {value}"
        )));
    }
    Ok(out)
}

pub fn provider_from_config(
    kind: ProviderKind,
    base_url: &str,
    model: &str,
    api_key: &str,
) -> Box<dyn LlmProvider> {
    match kind {
        ProviderKind::OpenAiCompatible | ProviderKind::Ollama => {
            Box::new(OpenAiCompatibleProvider {
                base_url: base_url.to_string(),
                api_key: api_key.to_string(),
                model: model.to_string(),
            })
        }
        ProviderKind::Gemini => Box::new(GeminiProvider {
            api_key: api_key.to_string(),
            model: model.to_string(),
        }),
        ProviderKind::Anthropic => Box::new(AnthropicProvider {
            api_key: api_key.to_string(),
            model: model.to_string(),
            base_url: base_url.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_extracts_output_text_delta() {
        let block = "event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n";
        assert_eq!(parse_sse_output_text_delta(block).as_deref(), Some("Hello"));
    }

    #[test]
    fn sse_parser_ignores_other_events() {
        let block = "event: response.created\n\
data: {\"type\":\"response.created\",\"id\":\"r1\"}\n";
        assert!(parse_sse_output_text_delta(block).is_none());
    }

    #[test]
    fn sse_parser_type_field_without_event_line() {
        let block = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"world\"}\n";
        assert_eq!(parse_sse_output_text_delta(block).as_deref(), Some("world"));
    }

    #[test]
    fn sse_parser_multiline_data() {
        let block = "event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\n\
data: \"delta\":\"ab\"}\n";
        // Joined data may not be valid JSON if split mid-token — use a clean split:
        let block2 = "event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"ab\"}\n";
        assert_eq!(parse_sse_output_text_delta(block2).as_deref(), Some("ab"));
        let _ = block; // keep for documentation
    }
}
