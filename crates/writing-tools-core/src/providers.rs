use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
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
        let url = format!(
            "{}/chat/completions",
            self.base_url.trim_end_matches('/')
        );
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
            return Err(CoreError::Provider(format!(
                "HTTP {status}: {value}"
            )));
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
            return Err(CoreError::Provider(format!(
                "HTTP {status}: {value}"
            )));
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
        Err(CoreError::Provider(format!("unexpected Anthropic response: {value}")))
    }
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
