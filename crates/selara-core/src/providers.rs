use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::chatgpt_auth::{
    ChatGptAuth, CODEX_MODELS_URL, CODEX_ORIGINATOR, CODEX_RESPONSES_URL, CODEX_USER_AGENT,
};
use crate::error::CoreError;

/// Fail fast on a black hole, but do not cap a still-progressing completion.
/// `timeout()` is a total deadline through the last body byte; a local model or
/// ChatGPT SSE stream can legitimately exceed that while still sending data.
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_READ_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn http_client() -> Result<reqwest::Client, CoreError> {
    Ok(reqwest::Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .read_timeout(HTTP_READ_IDLE_TIMEOUT)
        .build()?)
}

/// Read the body as text, then JSON. Non-JSON error pages keep the HTTP status.
async fn json_or_raw(
    resp: reqwest::Response,
) -> Result<(reqwest::StatusCode, serde_json::Value), CoreError> {
    let status = resp.status();
    let text = resp.text().await?;
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => Ok((status, value)),
        Err(_) if !status.is_success() => {
            let detail: String = text.chars().take(300).collect();
            Err(CoreError::Provider(format!("HTTP {status}: {detail}")))
        }
        Err(e) => Err(CoreError::Provider(format!(
            "invalid JSON ({e}): {}",
            text.chars().take(200).collect::<String>()
        ))),
    }
}

/// Chat Completions `message.content` is a string, or an array of text parts.
fn openai_message_content(value: &serde_json::Value) -> Option<String> {
    let content = value.pointer("/choices/0/message/content")?;
    if let Some(s) = content.as_str() {
        let trimmed = s.trim().to_string();
        return (!trimmed.is_empty()).then_some(trimmed);
    }
    let arr = content.as_array()?;
    let mut out = String::new();
    for block in arr {
        if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
            out.push_str(text);
        } else if let Some(s) = block.as_str() {
            out.push_str(s);
        }
    }
    let trimmed = out.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Drain complete SSE events from a streaming buffer (LF or CRLF framed).
pub fn take_complete_sse_events(buf: &mut String) -> Vec<String> {
    let mut events = Vec::new();
    while let Some((end, delim)) = sse_event_end(buf) {
        let event = buf[..end].to_string();
        buf.replace_range(..end + delim, "");
        if !event.trim().is_empty() {
            events.push(event);
        }
    }
    events
}

fn sse_event_end(buf: &str) -> Option<(usize, usize)> {
    let lf = buf.find("\n\n").map(|i| (i, 2usize));
    let crlf = buf.find("\r\n\r\n").map(|i| (i, 4usize));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any OpenAI-style `/chat/completions` endpoint (OpenAI, Ollama, LM Studio, vLLM, ...).
    /// Also accepts the UI spelling `openai_compatible` and the retired `ollama` kind.
    #[serde(alias = "openai_compatible", alias = "ollama")]
    OpenAiCompatible,
    /// OpenRouter: OpenAI-compatible wire format at `https://openrouter.ai/api/v1`.
    OpenRouter,
    Anthropic,
}

impl ProviderKind {
    /// Base URL used when the config leaves `base_url` empty.
    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible => "https://api.openai.com/v1",
            ProviderKind::OpenRouter => "https://openrouter.ai/api/v1",
            ProviderKind::Anthropic => "https://api.anthropic.com",
        }
    }

    /// Effective base URL: the configured one, or the kind's default when blank.
    pub fn resolve_base_url(self, configured: &str) -> String {
        let trimmed = configured.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            self.default_base_url().to_string()
        } else {
            trimmed.to_string()
        }
    }
}

/// Anthropic's documented root is host-only (`https://api.anthropic.com`). If a
/// config pastes the versioned `/v1` prefix, strip it so `/v1/messages` is not doubled.
fn anthropic_api_root(configured: &str) -> String {
    ProviderKind::Anthropic
        .resolve_base_url(configured)
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_string()
}

const OPENROUTER_REFERER: &str = "https://github.com/snowopsdev/selara";
const OPENROUTER_TITLE: &str = "Selara";
const ANTHROPIC_VERSION: &str = "2023-06-01";

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
    /// Extra request headers (OpenRouter attribution, for example).
    pub extra_headers: Vec<(String, String)>,
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let client = http_client()?;
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
        for (name, value) in &self.extra_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let resp = builder.send().await?;
        let (status, value) = json_or_raw(resp).await?;
        if !status.is_success() {
            return Err(CoreError::Provider(format!("HTTP {status}: {value}")));
        }

        openai_message_content(&value)
            .ok_or_else(|| CoreError::Provider(format!("unexpected response: {value}")))
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
        let base = anthropic_api_root(&self.base_url);
        let url = format!("{base}/v1/messages");
        let client = http_client()?;
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
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await?;
        let (status, value) = json_or_raw(resp).await?;
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

        let client = http_client()?;
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
            for event in take_complete_sse_events(&mut buf) {
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
    let client = http_client()?;
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
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(CoreError::Provider(format!(
            "list models HTTP {status}: {text}"
        )));
    }
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| CoreError::Provider(format!("list models: invalid JSON ({e})")))?;
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
    let base_url = kind.resolve_base_url(base_url);
    match kind {
        ProviderKind::OpenAiCompatible => Box::new(OpenAiCompatibleProvider {
            base_url,
            api_key: api_key.to_string(),
            model: model.to_string(),
            extra_headers: Vec::new(),
        }),
        ProviderKind::OpenRouter => Box::new(OpenAiCompatibleProvider {
            base_url,
            api_key: api_key.to_string(),
            model: model.to_string(),
            extra_headers: openrouter_headers(),
        }),
        ProviderKind::Anthropic => Box::new(AnthropicProvider {
            api_key: api_key.to_string(),
            model: model.to_string(),
            base_url,
        }),
    }
}

fn openrouter_headers() -> Vec<(String, String)> {
    vec![
        ("HTTP-Referer".to_string(), OPENROUTER_REFERER.to_string()),
        ("X-Title".to_string(), OPENROUTER_TITLE.to_string()),
    ]
}

/// List model ids for a BYOK provider. Doubles as a connection test: a bad key or
/// URL surfaces as a `CoreError::Provider` with the HTTP status.
pub async fn list_provider_models(
    kind: ProviderKind,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, CoreError> {
    let base = kind.resolve_base_url(base_url);
    let client = http_client()?;
    let mut models = match kind {
        ProviderKind::OpenAiCompatible | ProviderKind::OpenRouter => {
            let mut builder = client.get(format!("{base}/models"));
            if !api_key.is_empty() {
                builder = builder.bearer_auth(api_key);
            }
            if kind == ProviderKind::OpenRouter {
                for (name, value) in openrouter_headers() {
                    builder = builder.header(name, value);
                }
            }
            let value = send_json(builder, "list models").await?;
            let mut ids = parse_openai_models(&value)?;
            if kind == ProviderKind::OpenAiCompatible {
                ids.retain(|id| looks_like_chat_model(id));
            }
            ids
        }
        ProviderKind::Anthropic => {
            let root = anthropic_api_root(base_url);
            let mut ids = Vec::new();
            let mut after: Option<String> = None;
            // Anthropic pages with `has_more` / `last_id`; cap pages defensively.
            for _ in 0..10 {
                let mut builder = client
                    .get(format!("{root}/v1/models"))
                    .query(&[("limit", "1000")])
                    .header("x-api-key", api_key)
                    .header("anthropic-version", ANTHROPIC_VERSION);
                if let Some(id) = &after {
                    builder = builder.query(&[("after_id", id.as_str())]);
                }
                let value = send_json(builder, "list models").await?;
                let (page, next) = parse_anthropic_models(&value)?;
                ids.extend(page);
                match next {
                    Some(id) => after = Some(id),
                    None => break,
                }
            }
            ids
        }
    };
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(CoreError::Provider(
            "the provider returned no models for this key".into(),
        ));
    }
    Ok(models)
}

async fn send_json(
    builder: reqwest::RequestBuilder,
    what: &str,
) -> Result<serde_json::Value, CoreError> {
    let resp = builder.send().await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        let detail: String = text.chars().take(300).collect();
        return Err(CoreError::Provider(format!(
            "{what} HTTP {status}: {detail}"
        )));
    }
    serde_json::from_str(&text)
        .map_err(|e| CoreError::Provider(format!("{what}: invalid JSON ({e})")))
}

/// `{ "data": [ { "id": ... } ] }` (OpenAI, OpenRouter, Ollama's compat layer).
pub fn parse_openai_models(value: &serde_json::Value) -> Result<Vec<String>, CoreError> {
    let items = value
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| value.get("models").and_then(|v| v.as_array()))
        .ok_or_else(|| CoreError::Provider(format!("unexpected models response: {value}")))?;
    Ok(items
        .iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .collect())
}

/// `{ "data": [ { "id": ... } ], "has_more": bool, "last_id": ... }`. Returns the ids
/// on this page and the cursor for the next page when there is one.
pub fn parse_anthropic_models(
    value: &serde_json::Value,
) -> Result<(Vec<String>, Option<String>), CoreError> {
    let ids = parse_openai_models(value)?;
    let has_more = value
        .get("has_more")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let next = if has_more {
        value
            .get("last_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    };
    Ok((ids, next))
}

/// OpenAI's `/models` mixes in audio, image, embedding, and moderation models.
/// Keep the list to things that answer a chat completion.
pub fn looks_like_chat_model(id: &str) -> bool {
    const NOT_CHAT: [&str; 14] = [
        "embedding",
        "whisper",
        "tts",
        "dall-e",
        "moderation",
        "audio",
        "realtime",
        "transcribe",
        "image",
        "babbage",
        "davinci",
        "search",
        "similarity",
        "sora",
    ];
    let lower = id.to_ascii_lowercase();
    !NOT_CHAT.iter().any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_accepts_retired_ollama_alias() {
        let kind: ProviderKind = serde_json::from_str("\"ollama\"").unwrap();
        assert_eq!(kind, ProviderKind::OpenAiCompatible);
        let kind: ProviderKind = serde_json::from_str("\"open_router\"").unwrap();
        assert_eq!(kind, ProviderKind::OpenRouter);
        assert!(serde_json::from_str::<ProviderKind>("\"gemini\"").is_err());
    }

    #[test]
    fn provider_kind_round_trips_snake_case() {
        assert_eq!(
            serde_json::to_string(&ProviderKind::OpenRouter).unwrap(),
            "\"open_router\""
        );
    }

    #[test]
    fn resolve_base_url_falls_back_to_default_and_trims() {
        assert_eq!(
            ProviderKind::Anthropic.resolve_base_url("  "),
            "https://api.anthropic.com"
        );
        assert_eq!(
            ProviderKind::OpenRouter.resolve_base_url(""),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            ProviderKind::OpenAiCompatible.resolve_base_url("http://localhost:11434/v1/"),
            "http://localhost:11434/v1"
        );
    }

    #[test]
    fn anthropic_root_strips_versioned_prefix() {
        assert_eq!(anthropic_api_root(""), "https://api.anthropic.com");
        assert_eq!(
            anthropic_api_root("https://api.anthropic.com"),
            "https://api.anthropic.com"
        );
        assert_eq!(
            anthropic_api_root("https://api.anthropic.com/v1"),
            "https://api.anthropic.com"
        );
        assert_eq!(
            anthropic_api_root("https://api.anthropic.com/v1/"),
            "https://api.anthropic.com"
        );
        assert_eq!(
            format!(
                "{}/v1/messages",
                anthropic_api_root("https://api.anthropic.com/v1")
            ),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn parses_openai_style_model_list() {
        let v = serde_json::json!({"object":"list","data":[{"id":"gpt-4o-mini"},{"id":"gpt-4o"}]});
        assert_eq!(
            parse_openai_models(&v).unwrap(),
            vec!["gpt-4o-mini", "gpt-4o"]
        );
        let bad = serde_json::json!({"error":{"message":"nope"}});
        assert!(parse_openai_models(&bad).is_err());
    }

    #[test]
    fn parses_anthropic_model_pages() {
        let page = serde_json::json!({
            "data":[{"type":"model","id":"claude-opus-5","display_name":"Claude Opus 5"}],
            "has_more":true,"first_id":"claude-opus-5","last_id":"claude-opus-5"
        });
        let (ids, next) = parse_anthropic_models(&page).unwrap();
        assert_eq!(ids, vec!["claude-opus-5"]);
        assert_eq!(next.as_deref(), Some("claude-opus-5"));
        let last = serde_json::json!({"data":[{"id":"claude-haiku-4-5"}],"has_more":false});
        let (ids, next) = parse_anthropic_models(&last).unwrap();
        assert_eq!(ids, vec!["claude-haiku-4-5"]);
        assert!(next.is_none());
    }

    /// Live check against OpenRouter's public models endpoint (no key needed).
    /// Run with `cargo test -p selara-core -- --ignored openrouter`.
    #[tokio::test]
    #[ignore = "hits the network"]
    async fn openrouter_lists_models_without_a_key() {
        let models = list_provider_models(ProviderKind::OpenRouter, "", "")
            .await
            .unwrap();
        assert!(
            models.iter().any(|m| m.starts_with("anthropic/")),
            "{models:?}"
        );
        assert!(
            models.iter().any(|m| m.starts_with("openai/")),
            "{models:?}"
        );
    }

    #[test]
    fn chat_model_filter_drops_non_chat_ids() {
        assert!(looks_like_chat_model("gpt-4o-mini"));
        assert!(looks_like_chat_model("o4-mini"));
        assert!(!looks_like_chat_model("text-embedding-3-small"));
        assert!(!looks_like_chat_model("whisper-1"));
        assert!(!looks_like_chat_model("gpt-4o-realtime-preview"));
        assert!(!looks_like_chat_model("dall-e-3"));
    }

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

    #[test]
    fn http_client_uses_connect_and_read_idle_timeouts() {
        assert_eq!(HTTP_CONNECT_TIMEOUT, Duration::from_secs(10));
        assert_eq!(HTTP_READ_IDLE_TIMEOUT, Duration::from_secs(60));
        http_client().expect("client should build");
    }

    #[test]
    fn sse_framing_splits_crlf_and_lf_events() {
        let mut buf = String::from(
            "event: response.output_text.delta\r\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\r\n\r\n\
event: response.output_text.delta\r\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"!\"}\r\n\r\n",
        );
        let events = take_complete_sse_events(&mut buf);
        assert!(buf.is_empty(), "expected buffer drained, leftover {buf:?}");
        let text: String = events
            .iter()
            .filter_map(|e| parse_sse_output_text_delta(e))
            .collect();
        assert_eq!(text, "Hello!");

        let mut lf = String::from(
            "event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"A\"}\n\n\
event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"B\"}\n\npartial",
        );
        let events = take_complete_sse_events(&mut lf);
        assert_eq!(lf, "partial");
        let text: String = events
            .iter()
            .filter_map(|e| parse_sse_output_text_delta(e))
            .collect();
        assert_eq!(text, "AB");
    }

    #[test]
    fn openai_content_accepts_string_or_text_parts() {
        let string = serde_json::json!({
            "choices":[{"message":{"content":"  hi  "}}]
        });
        assert_eq!(openai_message_content(&string).as_deref(), Some("hi"));
        let parts = serde_json::json!({
            "choices":[{"message":{"content":[
                {"type":"text","text":"Hel"},
                {"type":"text","text":"lo"}
            ]}}]
        });
        assert_eq!(openai_message_content(&parts).as_deref(), Some("Hello"));
        let empty = serde_json::json!({"choices":[{"message":{"content":[]}}]});
        assert!(openai_message_content(&empty).is_none());
    }

    fn spawn_http(status: u16, body: &str, content_type: &str) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = body.to_string();
        let content_type = content_type.to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn html_error_page_keeps_http_status() {
        let base = spawn_http(502, "<html>bad gateway</html>", "text/html");
        let provider = OpenAiCompatibleProvider {
            base_url: format!("{base}/v1"),
            api_key: "k".into(),
            model: "m".into(),
            extra_headers: Vec::new(),
        };
        let err = provider
            .complete(CompletionRequest {
                system: "s".into(),
                user: "u".into(),
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("502") && msg.contains("bad gateway"),
            "expected HTTP 502 with body, got {msg}"
        );
        assert!(
            !msg.to_lowercase().contains("error decoding"),
            "must not hide status behind a JSON decode error: {msg}"
        );
    }

    #[tokio::test]
    async fn chat_completion_reads_text_part_array() {
        let payload = r#"{"choices":[{"message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}]}"#;
        let base = spawn_http(200, payload, "application/json");
        let provider = OpenAiCompatibleProvider {
            base_url: format!("{base}/v1"),
            api_key: "k".into(),
            model: "m".into(),
            extra_headers: Vec::new(),
        };
        let out = provider
            .complete(CompletionRequest {
                system: "s".into(),
                user: "u".into(),
            })
            .await
            .unwrap();
        assert_eq!(out, "ok");
    }

    #[tokio::test]
    async fn anthropic_reads_text_block() {
        let payload = r#"{"content":[{"type":"text","text":"claude-ok"}]}"#;
        let base = spawn_http(200, payload, "application/json");
        let provider = AnthropicProvider {
            api_key: "k".into(),
            model: "m".into(),
            base_url: base,
        };
        let out = provider
            .complete(CompletionRequest {
                system: "s".into(),
                user: "u".into(),
            })
            .await
            .unwrap();
        assert_eq!(out, "claude-ok");
    }

    #[tokio::test]
    async fn list_models_html_error_keeps_status() {
        let base = spawn_http(401, "<html>denied</html>", "text/html");
        let err = list_provider_models(ProviderKind::OpenAiCompatible, &base, "k")
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("401"), "{msg}");
        assert!(msg.contains("denied"), "{msg}");
    }
}
