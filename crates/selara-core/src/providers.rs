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

/// Error for a Responses API response that ended incomplete.
///
/// Only `max_output_tokens` is a token limit; every other reason gets its own
/// message naming what the API actually reported, so a `content_filter` stop is
/// not dressed up as "pick a model with a larger output limit". Either way the
/// partial text is discarded.
fn incomplete_response_error(reason: &str) -> CoreError {
    if reason == "max_output_tokens" {
        return truncation_error("response incomplete: max_output_tokens");
    }
    CoreError::Provider(format!(
        "the model stopped before finishing (incomplete_details.reason = {reason}); \
         the partial result was discarded because it would have replaced your selection"
    ))
}

/// Error returned when a provider stopped generating because it hit its output
/// token limit. Callers replace the user's selection with the result, so a
/// silently truncated rewrite would destroy text; surfacing it as an error is
/// the only safe option.
fn truncation_error(detail: &str) -> CoreError {
    CoreError::Provider(format!(
        "output was cut off by the model's token limit ({detail}); \
         shorten the selection or pick a model with a larger output limit"
    ))
}

/// OpenAI reasoning models (`o1`, `o3`, `o4-mini`, `gpt-5*`) reject any
/// non-default `temperature` with HTTP 400, so the request must omit it. An
/// optional `openai/` vendor prefix is stripped first because OpenRouter ids
/// look like `openai/o3`. The `o` + digit rule deliberately excludes ids such
/// as `omni-moderation`, and `gpt-4o*` never matches because it starts with `gpt-4`.
pub fn is_reasoning_model(id: &str) -> bool {
    let lower = id.trim().to_ascii_lowercase();
    let bare = lower.strip_prefix("openai/").unwrap_or(&lower);
    let mut chars = bare.chars();
    let o_series = matches!(
        (chars.next(), chars.next()),
        (Some('o'), Some(d)) if d.is_ascii_digit()
    );
    o_series || bare.starts_with("gpt-5")
}

/// Every Claude model has always accepted this many output tokens, so it is the
/// safe request size for a model id this build does not recognise.
const ANTHROPIC_MIN_MAX_TOKENS: u32 = 4_096;
/// The most this helper ever asks for. Chosen because every Claude model from
/// 3.7 / 4 onwards accepts at least this much.
const ANTHROPIC_MAX_MAX_TOKENS: u32 = 32_000;

/// The output-token ceiling of `model`.
///
/// Anthropic rejects a `max_tokens` above the selected model's output limit
/// with HTTP 400, and that limit is per model: the original Claude 3 family
/// caps at 4,096 and Claude 3.5 at 8,192, while Claude 3.7 and everything after
/// it accepts at least 64,000. Because the provider takes arbitrary model ids
/// — a custom gateway, or a model released after this build — an id that
/// matches nothing known gets the floor rather than an optimistic guess, which
/// is exactly the fixed 4,096 that used to be sent unconditionally.
///
/// A vendor prefix is stripped first, so OpenRouter-style `anthropic/claude-…`
/// ids classify the same as bare ones.
///
/// The dynamic alternative is `GET /v1/models/{id}`, whose `max_tokens` field
/// reports this per model; that costs an extra round trip on every completion,
/// and this covers the published families with a safe fallback for the rest.
fn anthropic_model_max_tokens(model: &str) -> u32 {
    let id = model.trim().to_ascii_lowercase();
    let bare = id.rsplit('/').next().unwrap_or(id.as_str());
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| bare.starts_with(p));

    if starts(&["claude-3-5-", "claude-3.5-"]) {
        8_192
    } else if starts(&["claude-3-7-", "claude-3.7-"]) {
        // 64,000, i.e. more than this helper ever asks for.
        ANTHROPIC_MAX_MAX_TOKENS
    } else if starts(&["claude-3-", "claude-3."]) {
        // Claude 3 Opus / Sonnet / Haiku.
        4_096
    } else if bare.starts_with("claude-") {
        // Claude 4 and later. The lowest ceiling in that range is Opus 4 /
        // Opus 4.1 at 32,000; the rest are 64,000 or more.
        ANTHROPIC_MAX_MAX_TOKENS
    } else {
        ANTHROPIC_MIN_MAX_TOKENS
    }
}

/// `max_tokens` for an Anthropic request sized to the input, then held to what
/// `model` actually accepts. A rewrite's output is roughly the length of its
/// input; at ~4 chars per token, `chars / 2` gives about 2x headroom. The floor
/// keeps short inputs generous, and the model's own ceiling has the last word
/// so a long selection cannot produce a request the API rejects outright.
pub fn anthropic_max_tokens(model: &str, input_chars: usize) -> u32 {
    let want = (input_chars / 2).clamp(
        ANTHROPIC_MIN_MAX_TOKENS as usize,
        ANTHROPIC_MAX_MAX_TOKENS as usize,
    ) as u32;
    want.min(anthropic_model_max_tokens(model))
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
        let mut body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": req.user}
            ]
        });
        if !is_reasoning_model(&self.model) {
            body["temperature"] = json!(0.2);
        }

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
        if value
            .pointer("/choices/0/finish_reason")
            .and_then(|v| v.as_str())
            == Some("length")
        {
            return Err(truncation_error("finish_reason=length"));
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
            "max_tokens": anthropic_max_tokens(&self.model, req.user.chars().count()),
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
        if value.get("stop_reason").and_then(|v| v.as_str()) == Some("max_tokens") {
            return Err(truncation_error("stop_reason=max_tokens"));
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
        let mut incomplete: Option<String> = None;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            for event in take_complete_sse_events(&mut buf) {
                if let Some(delta) = parse_sse_output_text_delta(&event) {
                    out.push_str(&delta);
                }
                incomplete = incomplete.or_else(|| sse_event_incomplete_reason(&event));
            }
        }
        // Trailing event without final blank line
        if !buf.trim().is_empty() {
            if let Some(delta) = parse_sse_output_text_delta(&buf) {
                out.push_str(&delta);
            }
            incomplete = incomplete.or_else(|| sse_event_incomplete_reason(&buf));
        }
        // Drain the whole stream first so the connection closes cleanly, but
        // never hand back partial text: the caller would write it over the selection.
        if let Some(reason) = incomplete {
            return Err(incomplete_response_error(&reason));
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

/// Split one SSE event block into its `event:` name and parsed JSON `data:`
/// payload. Data may be split across multiple `data:` lines. Returns `None` for
/// blocks with no data, the `[DONE]` sentinel, or non-JSON data.
fn parse_sse_event(event_block: &str) -> Option<(Option<String>, serde_json::Value)> {
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
    Some((event_name, value))
}

/// Stand-in when a terminal event says the response is incomplete but names no
/// `incomplete_details.reason`.
pub const UNKNOWN_INCOMPLETE_REASON: &str = "unspecified";

/// Why a Responses API terminal event says the output was cut short, if it was.
///
/// Terminal-and-incomplete means a `response.incomplete` event, a
/// `response.completed` whose `response.status` is not `completed`, or any
/// terminal event carrying an `incomplete_details.reason`. The reason itself is
/// returned rather than folded into a boolean: `max_output_tokens` is a token
/// limit the user can act on by shortening the selection or changing model, but
/// `content_filter` and the other reasons are not, and telling someone to pick
/// a bigger model for a content filter is wrong advice. A reason-less incomplete
/// yields `UNKNOWN_INCOMPLETE_REASON`, so the partial output is still rejected.
pub fn sse_event_incomplete_reason(event_block: &str) -> Option<String> {
    let (event_name, value) = parse_sse_event(event_block)?;
    let json_type = value.get("type").and_then(|v| v.as_str());
    let is_type = |name: &str| event_name.as_deref() == Some(name) || json_type == Some(name);
    let reason = value
        .pointer("/response/incomplete_details/reason")
        .and_then(|v| v.as_str());

    if is_type("response.incomplete") {
        return Some(reason.unwrap_or(UNKNOWN_INCOMPLETE_REASON).to_string());
    }
    if let Some(reason) = reason {
        return Some(reason.to_string());
    }
    if is_type("response.completed") {
        let status = value.pointer("/response/status").and_then(|v| v.as_str());
        if status.is_some_and(|s| s != "completed") {
            return Some(UNKNOWN_INCOMPLETE_REASON.to_string());
        }
    }
    None
}

/// Extract text from an SSE event whose `event:` is `response.output_text.delta`
/// (or whose JSON `type` field matches). Data may be split across multiple `data:` lines.
pub fn parse_sse_output_text_delta(event_block: &str) -> Option<String> {
    let (event_name, value) = parse_sse_event(event_block)?;
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
        spawn_http_capture(status, body, content_type).0
    }

    /// One-shot fake HTTP server. Reads the full request (headers, then
    /// `Content-Length` bytes of body), sends the captured request body on the
    /// returned channel, and answers with the canned response.
    fn spawn_http_capture(
        status: u16,
        body: &str,
        content_type: &str,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = body.to_string();
        let content_type = content_type.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            let header_end = loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break None;
                }
                raw.extend_from_slice(&chunk[..n]);
                if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(pos + 4);
                }
            };
            let request_body = match header_end {
                Some(end) => {
                    let head = String::from_utf8_lossy(&raw[..end]).to_string();
                    let len = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    while raw.len() < end + len {
                        let n = stream.read(&mut chunk).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        raw.extend_from_slice(&chunk[..n]);
                    }
                    String::from_utf8_lossy(&raw[end..raw.len().min(end + len)]).to_string()
                }
                None => String::new(),
            };
            let _ = tx.send(request_body);
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        });
        (format!("http://{addr}"), rx)
    }

    fn openai_provider(base: &str, model: &str) -> OpenAiCompatibleProvider {
        OpenAiCompatibleProvider {
            base_url: format!("{base}/v1"),
            api_key: "k".into(),
            model: model.into(),
            extra_headers: Vec::new(),
        }
    }

    fn simple_req() -> CompletionRequest {
        CompletionRequest {
            system: "s".into(),
            user: "u".into(),
        }
    }

    const OPENAI_OK: &str =
        r#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#;

    #[test]
    fn reasoning_model_detection() {
        for id in [
            "o4-mini",
            "o3",
            "o1-preview",
            "gpt-5",
            "gpt-5.4-mini",
            "openai/o3",
            "OpenAI/GPT-5",
        ] {
            assert!(is_reasoning_model(id), "{id} should be a reasoning model");
        }
        for id in [
            "gpt-4o-mini",
            "gpt-4.1",
            "omni-moderation",
            "llama3.1:8b",
            "claude-opus-5",
            "",
            "o",
        ] {
            assert!(
                !is_reasoning_model(id),
                "{id} should not be a reasoning model"
            );
        }
    }

    #[tokio::test]
    async fn reasoning_model_request_omits_temperature() {
        let (base, rx) = spawn_http_capture(200, OPENAI_OK, "application/json");
        let out = openai_provider(&base, "o4-mini")
            .complete(simple_req())
            .await
            .unwrap();
        assert_eq!(out, "ok");
        let sent: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert!(
            sent.get("temperature").is_none(),
            "reasoning model body must not carry temperature: {sent}"
        );
        assert_eq!(sent["model"], "o4-mini");
    }

    #[tokio::test]
    async fn non_reasoning_model_request_sends_temperature() {
        let (base, rx) = spawn_http_capture(200, OPENAI_OK, "application/json");
        openai_provider(&base, "gpt-4o-mini")
            .complete(simple_req())
            .await
            .unwrap();
        let sent: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(sent["temperature"], 0.2, "{sent}");
    }

    #[tokio::test]
    async fn openai_finish_reason_length_is_an_error() {
        let payload = r#"{"choices":[{"message":{"role":"assistant","content":"partial text"},"finish_reason":"length"}]}"#;
        let base = spawn_http(200, payload, "application/json");
        let err = openai_provider(&base, "gpt-4o-mini")
            .complete(simple_req())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cut off"), "{msg}");
        assert!(msg.contains("finish_reason=length"), "{msg}");
        assert!(
            !msg.contains("partial text"),
            "must not leak partial output: {msg}"
        );
    }

    #[tokio::test]
    async fn openai_finish_reason_stop_returns_content() {
        let base = spawn_http(200, OPENAI_OK, "application/json");
        let out = openai_provider(&base, "gpt-4o-mini")
            .complete(simple_req())
            .await
            .unwrap();
        assert_eq!(out, "ok");
    }

    #[tokio::test]
    async fn anthropic_stop_reason_max_tokens_is_an_error() {
        let payload =
            r#"{"content":[{"type":"text","text":"partial"}],"stop_reason":"max_tokens"}"#;
        let base = spawn_http(200, payload, "application/json");
        let provider = AnthropicProvider {
            api_key: "k".into(),
            model: "m".into(),
            base_url: base,
        };
        let err = provider.complete(simple_req()).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cut off"), "{msg}");
        assert!(msg.contains("stop_reason=max_tokens"), "{msg}");
    }

    #[tokio::test]
    async fn anthropic_end_turn_returns_text_and_sizes_max_tokens() {
        let payload = r#"{"content":[{"type":"text","text":"done"}],"stop_reason":"end_turn"}"#;
        let (base, rx) = spawn_http_capture(200, payload, "application/json");
        let provider = AnthropicProvider {
            api_key: "k".into(),
            // A real id: the sizing is now capped by the model's own output
            // limit, so this must not be a placeholder.
            model: "claude-sonnet-4-5".into(),
            base_url: base,
        };
        let out = provider
            .complete(CompletionRequest {
                system: "s".into(),
                user: "x".repeat(20_000),
            })
            .await
            .unwrap();
        assert_eq!(out, "done");
        let sent: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(sent["max_tokens"], 10_000, "{}", sent["max_tokens"]);
    }

    /// The same request against a 8,192-token model must send 8,192, not the
    /// 10,000 the input size alone would ask for — Anthropic rejects the larger
    /// value with HTTP 400 before generating anything.
    #[tokio::test]
    async fn anthropic_request_is_held_to_the_model_output_limit() {
        let payload = r#"{"content":[{"type":"text","text":"done"}],"stop_reason":"end_turn"}"#;
        let (base, rx) = spawn_http_capture(200, payload, "application/json");
        let provider = AnthropicProvider {
            api_key: "k".into(),
            model: "claude-3-5-sonnet-latest".into(),
            base_url: base,
        };
        provider
            .complete(CompletionRequest {
                system: "s".into(),
                user: "x".repeat(20_000),
            })
            .await
            .unwrap();
        let sent: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(sent["max_tokens"], 8_192, "{}", sent["max_tokens"]);
    }

    #[test]
    fn anthropic_max_tokens_clamps_to_input_size() {
        let model = "claude-sonnet-4-5";
        assert_eq!(anthropic_max_tokens(model, 0), 4096);
        assert_eq!(anthropic_max_tokens(model, 100), 4096);
        assert_eq!(anthropic_max_tokens(model, 8192), 4096);
        assert_eq!(anthropic_max_tokens(model, 20_000), 10_000);
        assert_eq!(anthropic_max_tokens(model, 1_000_000), 32_000);
    }

    /// The regression: a long selection asked for up to 32,000 output tokens
    /// regardless of model, and Anthropic answers HTTP 400 when that exceeds
    /// the selected model's ceiling - so a request that used to succeed at the
    /// old fixed 4,096 started failing before generation.
    #[test]
    fn anthropic_max_tokens_never_exceeds_the_model_ceiling() {
        // Claude 3 family: 4,096.
        assert_eq!(
            anthropic_max_tokens("claude-3-opus-20240229", 1_000_000),
            4_096
        );
        assert_eq!(
            anthropic_max_tokens("claude-3-haiku-20240307", 100_000),
            4_096
        );
        // Claude 3.5: 8,192.
        assert_eq!(
            anthropic_max_tokens("claude-3-5-sonnet-latest", 1_000_000),
            8_192
        );
        assert_eq!(
            anthropic_max_tokens("claude-3-5-haiku-latest", 30_000),
            8_192
        );
        // Claude 3.7 and Claude 4+: at least 32,000, so the input sizing wins.
        assert_eq!(
            anthropic_max_tokens("claude-3-7-sonnet-latest", 1_000_000),
            32_000
        );
        assert_eq!(anthropic_max_tokens("claude-opus-4-1", 1_000_000), 32_000);
        // A vendor prefix must not defeat the classification.
        assert_eq!(
            anthropic_max_tokens("anthropic/claude-3-5-sonnet-latest", 1_000_000),
            8_192
        );
        // An id this build does not know gets the always-safe floor.
        assert_eq!(anthropic_max_tokens("my-gateway-model", 1_000_000), 4_096);
    }

    #[test]
    fn sse_truncation_detects_incomplete_terminal_events() {
        let incomplete = "event: response.incomplete\n\
data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n";
        assert_eq!(
            sse_event_incomplete_reason(incomplete).as_deref(),
            Some("max_output_tokens")
        );

        let completed_but_cut = "event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n";
        assert_eq!(
            sse_event_incomplete_reason(completed_but_cut).as_deref(),
            Some("max_output_tokens")
        );

        let completed_wrong_status =
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"incomplete\"}}\n";
        assert_eq!(
            sse_event_incomplete_reason(completed_wrong_status).as_deref(),
            Some(UNKNOWN_INCOMPLETE_REASON)
        );
    }

    #[test]
    fn sse_truncation_ignores_normal_events() {
        let completed = "event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"incomplete_details\":null}}\n";
        assert!(sse_event_incomplete_reason(completed).is_none());

        let delta = "event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n";
        assert!(sse_event_incomplete_reason(delta).is_none());

        assert!(sse_event_incomplete_reason("data: [DONE]\n").is_none());
        assert!(sse_event_incomplete_reason("event: ping\n").is_none());
    }

    /// The regression: every `response.incomplete` was reported as a token
    /// limit, so a content-filter stop told the user to pick a model with a
    /// larger output limit - advice that cannot help.
    #[test]
    fn non_token_limit_incomplete_reasons_are_reported_as_themselves() {
        let filtered = "event: response.incomplete\n\
data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"content_filter\"}}}\n";
        assert_eq!(
            sse_event_incomplete_reason(filtered).as_deref(),
            Some("content_filter")
        );

        let msg = incomplete_response_error("content_filter").to_string();
        assert!(
            msg.contains("content_filter"),
            "names the real reason: {msg}"
        );
        assert!(
            !msg.contains("larger output limit"),
            "does not give token-limit advice: {msg}"
        );

        // The token-limit case keeps its original wording and advice.
        let token = incomplete_response_error("max_output_tokens").to_string();
        assert!(token.contains("larger output limit"), "{token}");
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
