use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::chatgpt_auth::{
    ChatGptAuth, CODEX_MODELS_URL, CODEX_ORIGINATOR, CODEX_RESPONSES_URL, CODEX_USER_AGENT,
};
use crate::error::CoreError;
use crate::usage::{self, TokenUsage};

/// Fail fast on a black hole, but do not cap a still-progressing completion.
/// `timeout()` is a total deadline through the last body byte; a local model or
/// ChatGPT SSE stream can legitimately exceed that while still sending data.
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_READ_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Statuses worth one more try: rate limiting and upstream/gateway hiccups.
/// A 500 is deliberately absent because it usually means the request itself
/// is bad and would fail again.
const RETRYABLE_STATUSES: [u16; 4] = [429, 502, 503, 504];
/// Honour `Retry-After` only when it is short; anything longer is treated as
/// a normal transient wait so a hostile header cannot stall the hotkey.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);
const RETRY_BASE_DELAY: Duration = Duration::from_millis(1000);
const RETRY_JITTER_MS: u64 = 500;

/// One process-wide client so every provider call shares the connection pool
/// and the TLS configuration instead of rebuilding both per request. A build
/// failure is cached as the error text and returned on every call rather than
/// panicking; `reqwest::Error` is not `Clone`, hence the `String`.
static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

pub(crate) fn http_client() -> Result<reqwest::Client, CoreError> {
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(HTTP_CONNECT_TIMEOUT)
                .read_timeout(HTTP_READ_IDLE_TIMEOUT)
                .build()
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(|e| CoreError::Provider(format!("HTTP client: {e}")))
}

/// Send a request and retry it once on a transient failure: a connect or
/// timeout error, or a 429/502/503/504 response. Requests whose body cannot be
/// cloned (streams) are sent once. The second attempt's result is returned
/// as-is so callers keep shaping errors exactly as they did without retry.
pub(crate) async fn send_with_retry(
    builder: reqwest::RequestBuilder,
    what: &str,
) -> Result<reqwest::Response, CoreError> {
    let Some(retry) = builder.try_clone() else {
        return Ok(builder.send().await?);
    };
    let delay = match builder.send().await {
        Ok(resp) if RETRYABLE_STATUSES.contains(&resp.status().as_u16()) => {
            let status = resp.status();
            let delay = retry_delay(retry_after(&resp));
            tracing::warn!(%what, %status, ?delay, "retrying after transient HTTP status");
            delay
        }
        Ok(resp) => return Ok(resp),
        Err(e) if e.is_connect() || e.is_timeout() => {
            let delay = retry_delay(None);
            tracing::warn!(%what, error = %e, ?delay, "retrying after request error");
            delay
        }
        Err(e) => return Err(e.into()),
    };
    tokio::time::sleep(delay).await;
    Ok(retry.send().await?)
}

/// `Retry-After` in delay-seconds form, when present and no longer than
/// [`MAX_RETRY_AFTER`]. HTTP-date form is ignored (falls back to the default).
fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let secs = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    let wait = Duration::from_secs(secs);
    (wait <= MAX_RETRY_AFTER).then_some(wait)
}

/// The server's hint if usable, else the base delay plus 0..500 ms of jitter
/// taken from the clock so simultaneous retries do not line up.
fn retry_delay(hint: Option<Duration>) -> Duration {
    hint.unwrap_or_else(|| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        RETRY_BASE_DELAY + Duration::from_millis(nanos % RETRY_JITTER_MS)
    })
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

/// `max_tokens` for an Anthropic request sized to the input. A rewrite's output
/// is roughly the length of its input; at ~4 chars per token, `chars / 2` gives
/// about 2x headroom. The floor keeps short inputs generous and the ceiling
/// stays under current model output limits.
pub fn anthropic_max_tokens(input_chars: usize) -> u32 {
    (input_chars / 2).clamp(4096, 32_000) as u32
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

/// Receiver for streamed text fragments, see [`LlmProvider::complete_stream`].
/// A named alias so the `&str` stays higher-ranked (`for<'a>`) through the
/// `async_trait` rewrite, which would otherwise pin it to one lifetime.
pub type DeltaSink<'a> = dyn FnMut(&str) + Send + 'a;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError>;

    /// Stream the reply; `on_delta` receives each text fragment in order.
    /// Returns the full text.
    ///
    /// Fragments are forwarded exactly as the model sent them, before any
    /// trimming or truncation check, so a caller may see fragments and then an
    /// error; only the returned `String` is safe to write over a selection.
    /// The default buffers [`complete`](Self::complete) and delivers it as one
    /// fragment, so providers that cannot stream still fit the same call path.
    async fn complete_stream(
        &self,
        req: CompletionRequest,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<String, CoreError> {
        let out = self.complete(req).await?;
        on_delta(&out);
        Ok(out)
    }
}

/// Read an SSE response body to the end and hand every complete event block
/// to `on_event`, including a trailing block the server did not terminate
/// with a blank line. Framing (`take_complete_sse_events`) and UTF-8
/// reassembly live here so the three streaming providers only differ in how
/// they interpret an event. An `Err` from `on_event` stops reading and is
/// returned as-is.
async fn for_each_sse_event(
    resp: reqwest::Response,
    mut on_event: impl FnMut(&str) -> Result<(), CoreError> + Send,
) -> Result<(), CoreError> {
    let mut stream = resp.bytes_stream();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        pending.extend_from_slice(&chunk?);
        drain_utf8(&mut pending, &mut buf);
        for event in take_complete_sse_events(&mut buf) {
            on_event(&event)?;
        }
    }
    if !pending.is_empty() {
        buf.push_str(&String::from_utf8_lossy(&pending));
    }
    if !buf.trim().is_empty() {
        on_event(&buf)?;
    }
    Ok(())
}

/// Move the decodable prefix of `bytes` into `out`. A multi-byte character
/// split across two network chunks is left in `bytes` until its tail arrives
/// instead of being replaced with U+FFFD; genuinely invalid bytes are decoded
/// lossily so a bad server cannot stall the stream.
fn drain_utf8(bytes: &mut Vec<u8>, out: &mut String) {
    let valid = match std::str::from_utf8(bytes) {
        Ok(s) => {
            out.push_str(s);
            bytes.clear();
            return;
        }
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => bytes.len(),
    };
    out.push_str(&String::from_utf8_lossy(&bytes[..valid]));
    bytes.drain(..valid);
}

/// `Content-Type` says JSON: a server that ignored `"stream": true` and sent
/// one buffered reply (some OpenAI-compatible proxies do). Those responses are
/// parsed the non-streaming way and delivered as a single fragment.
fn is_json_response(resp: &reqwest::Response) -> bool {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.to_ascii_lowercase().contains("application/json"))
}

/// Shape a non-2xx reply exactly as the buffered providers do: the JSON error
/// body when there is one, otherwise the raw text with the status kept.
async fn http_error(resp: reqwest::Response) -> CoreError {
    match json_or_raw(resp).await {
        Ok((status, value)) => CoreError::Provider(format!("HTTP {status}: {value}")),
        Err(e) => e,
    }
}

pub struct OpenAiCompatibleProvider {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Extra request headers (OpenRouter attribution, for example).
    pub extra_headers: Vec<(String, String)>,
}

impl OpenAiCompatibleProvider {
    fn request(
        &self,
        req: &CompletionRequest,
        stream: bool,
    ) -> Result<reqwest::RequestBuilder, CoreError> {
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
        if stream {
            body["stream"] = json!(true);
            // Ask for a final usage-only chunk (OpenAI, vLLM; others ignore it).
            body["stream_options"] = json!({"include_usage": true});
        }

        let mut builder = client.post(url).json(&body);
        if stream {
            builder = builder.header("Accept", "text/event-stream");
        }
        if !self.api_key.is_empty() {
            builder = builder.bearer_auth(&self.api_key);
        }
        for (name, value) in &self.extra_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        Ok(builder)
    }

    /// Text of a buffered Chat Completions reply, or the truncation error.
    fn parse_response(value: &serde_json::Value) -> Result<String, CoreError> {
        if value
            .pointer("/choices/0/finish_reason")
            .and_then(|v| v.as_str())
            == Some("length")
        {
            return Err(truncation_error("finish_reason=length"));
        }
        openai_message_content(value)
            .ok_or_else(|| CoreError::Provider(format!("unexpected response: {value}")))
    }

    /// Ledger label: OpenRouter is the same wire format with attribution headers.
    fn usage_kind(&self) -> &'static str {
        if self.extra_headers.is_empty() {
            usage::KIND_OPENAI_COMPATIBLE
        } else {
            usage::KIND_OPENROUTER
        }
    }

    fn record_usage(&self, value: &serde_json::Value) {
        if let Some(u) = parse_openai_usage(value) {
            usage::record(self.usage_kind(), &self.model, u);
        }
    }
}

/// `usage.prompt_tokens` / `usage.completion_tokens` from a Chat Completions
/// reply or its final streamed chunk. `None` when the server sent no usage.
pub fn parse_openai_usage(value: &serde_json::Value) -> Option<TokenUsage> {
    let u = value.get("usage")?;
    if u.is_null() {
        return None;
    }
    Some(TokenUsage {
        input: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        output: u
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    })
}

/// `usage.input_tokens` / `usage.output_tokens` from a buffered Messages reply.
pub fn parse_anthropic_usage(value: &serde_json::Value) -> Option<TokenUsage> {
    let u = value.get("usage")?;
    if u.is_null() {
        return None;
    }
    Some(TokenUsage {
        input: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        output: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
        let builder = self.request(&req, false)?;
        let resp = send_with_retry(builder, "chat completion").await?;
        let (status, value) = json_or_raw(resp).await?;
        if !status.is_success() {
            return Err(CoreError::Provider(format!("HTTP {status}: {value}")));
        }
        self.record_usage(&value);
        Self::parse_response(&value)
    }

    async fn complete_stream(
        &self,
        req: CompletionRequest,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<String, CoreError> {
        let builder = self.request(&req, true)?;
        let resp = send_with_retry(builder, "chat completion").await?;
        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }
        if is_json_response(&resp) {
            let (_, value) = json_or_raw(resp).await?;
            self.record_usage(&value);
            let out = Self::parse_response(&value)?;
            on_delta(&out);
            return Ok(out);
        }

        let mut out = String::new();
        let mut truncated = false;
        let mut used: Option<TokenUsage> = None;
        for_each_sse_event(resp, |event| {
            let Some((_, value)) = parse_sse_event(event) else {
                return Ok(());
            };
            if let Some(err) = value.get("error") {
                return Err(CoreError::Provider(format!("stream error: {err}")));
            }
            // With `include_usage` the last chunk carries `usage` and no choices.
            if let Some(u) = parse_openai_usage(&value) {
                used = Some(u);
            }
            // Role-only and keep-alive chunks carry `content: null`; skip them.
            if let Some(text) = value
                .pointer("/choices/0/delta/content")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                out.push_str(text);
                on_delta(text);
            }
            if value
                .pointer("/choices/0/finish_reason")
                .and_then(|v| v.as_str())
                == Some("length")
            {
                truncated = true;
            }
            Ok(())
        })
        .await?;
        if let Some(u) = used {
            usage::record(self.usage_kind(), &self.model, u);
        }
        // Drain the whole stream first so the connection closes cleanly, but
        // never hand back partial text: the caller would write it over the selection.
        if truncated {
            return Err(truncation_error("finish_reason=length"));
        }
        let trimmed = out.trim().to_string();
        if trimmed.is_empty() {
            return Err(CoreError::Provider(
                "unexpected response: the stream carried no content deltas".into(),
            ));
        }
        Ok(trimmed)
    }
}

pub struct AnthropicProvider {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

impl AnthropicProvider {
    fn request(
        &self,
        req: &CompletionRequest,
        stream: bool,
    ) -> Result<reqwest::RequestBuilder, CoreError> {
        let base = anthropic_api_root(&self.base_url);
        let url = format!("{base}/v1/messages");
        let client = http_client()?;
        let mut body = json!({
            "model": self.model,
            "max_tokens": anthropic_max_tokens(req.user.chars().count()),
            "system": req.system,
            "messages": [
                {"role": "user", "content": req.user}
            ]
        });
        if stream {
            body["stream"] = json!(true);
        }
        let mut builder = client
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body);
        if stream {
            builder = builder.header("Accept", "text/event-stream");
        }
        Ok(builder)
    }

    /// Text of a buffered Messages reply, or the truncation error.
    fn parse_response(value: &serde_json::Value) -> Result<String, CoreError> {
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

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
        let builder = self.request(&req, false)?;
        let resp = send_with_retry(builder, "anthropic messages").await?;
        let (status, value) = json_or_raw(resp).await?;
        if !status.is_success() {
            return Err(CoreError::Provider(format!("HTTP {status}: {value}")));
        }
        if let Some(u) = parse_anthropic_usage(&value) {
            usage::record(usage::KIND_ANTHROPIC, &self.model, u);
        }
        Self::parse_response(&value)
    }

    async fn complete_stream(
        &self,
        req: CompletionRequest,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<String, CoreError> {
        let builder = self.request(&req, true)?;
        let resp = send_with_retry(builder, "anthropic messages").await?;
        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }
        if is_json_response(&resp) {
            let (_, value) = json_or_raw(resp).await?;
            if let Some(u) = parse_anthropic_usage(&value) {
                usage::record(usage::KIND_ANTHROPIC, &self.model, u);
            }
            let out = Self::parse_response(&value)?;
            on_delta(&out);
            return Ok(out);
        }

        let mut out = String::new();
        let mut truncated = false;
        // `message_start` carries input tokens, `message_delta` the output count.
        let mut used: Option<TokenUsage> = None;
        for_each_sse_event(resp, |event| {
            let Some((event_name, value)) = parse_sse_event(event) else {
                return Ok(());
            };
            let json_type = value.get("type").and_then(|v| v.as_str());
            let kind = event_name.as_deref().or(json_type).unwrap_or("");
            match kind {
                "content_block_delta" => {
                    if value.pointer("/delta/type").and_then(|v| v.as_str()) == Some("text_delta") {
                        if let Some(text) = value.pointer("/delta/text").and_then(|v| v.as_str()) {
                            out.push_str(text);
                            on_delta(text);
                        }
                    }
                }
                "message_start" => {
                    if let Some(n) = value
                        .pointer("/message/usage/input_tokens")
                        .and_then(|v| v.as_u64())
                    {
                        used.get_or_insert_with(TokenUsage::default).input = n;
                    }
                }
                "message_delta" => {
                    if value.pointer("/delta/stop_reason").and_then(|v| v.as_str())
                        == Some("max_tokens")
                    {
                        truncated = true;
                    }
                    if let Some(n) = value
                        .pointer("/usage/output_tokens")
                        .and_then(|v| v.as_u64())
                    {
                        used.get_or_insert_with(TokenUsage::default).output = n;
                    }
                }
                "error" => {
                    let message = value
                        .pointer("/error/message")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string());
                    return Err(CoreError::Provider(format!(
                        "Anthropic stream error: {message}"
                    )));
                }
                // message_start, content_block_start/stop, ping, message_stop:
                // nothing to extract; the body ends after message_stop.
                _ => {}
            }
            Ok(())
        })
        .await?;
        if let Some(u) = used {
            usage::record(usage::KIND_ANTHROPIC, &self.model, u);
        }
        if truncated {
            return Err(truncation_error("stop_reason=max_tokens"));
        }
        let trimmed = out.trim().to_string();
        if trimmed.is_empty() {
            return Err(CoreError::Provider(
                "unexpected Anthropic response: the stream carried no text deltas".into(),
            ));
        }
        Ok(trimmed)
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
    /// The Responses endpoint is always streamed; buffering is just a no-op sink.
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
        self.complete_stream(req, &mut |_: &str| {}).await
    }

    async fn complete_stream(
        &self,
        req: CompletionRequest,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<String, CoreError> {
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

        // Only the initial POST is retried; once the SSE stream is open a
        // failure mid-stream surfaces to the caller as before.
        let resp = send_with_retry(builder, "chatgpt codex responses").await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(CoreError::Provider(format!(
                "ChatGPT Codex HTTP {status}: {text}"
            )));
        }

        let mut out = String::new();
        let mut truncated = false;
        let mut used: Option<TokenUsage> = None;
        for_each_sse_event(resp, |event| {
            if let Some(delta) = parse_sse_output_text_delta(event) {
                out.push_str(&delta);
                on_delta(&delta);
            }
            if let Some(u) = parse_sse_response_usage(event) {
                used = Some(u);
            }
            truncated |= sse_event_marks_truncation(event);
            Ok(())
        })
        .await?;
        if let Some(u) = used {
            usage::record(usage::KIND_CHATGPT_CODEX, &self.model, u);
        }
        // Drain the whole stream first so the connection closes cleanly, but
        // never hand back partial text: the caller would write it over the selection.
        if truncated {
            return Err(truncation_error("response incomplete: max_output_tokens"));
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

/// True when a Responses API terminal event says the output was cut short:
/// a `response.incomplete` event, a `response.completed` whose `response.status`
/// is not `completed`, or any terminal event whose
/// `response.incomplete_details.reason` is `max_output_tokens`.
pub fn sse_event_marks_truncation(event_block: &str) -> bool {
    let Some((event_name, value)) = parse_sse_event(event_block) else {
        return false;
    };
    let json_type = value.get("type").and_then(|v| v.as_str());
    let is_type = |name: &str| event_name.as_deref() == Some(name) || json_type == Some(name);
    if is_type("response.incomplete") {
        return true;
    }
    let reason = value
        .pointer("/response/incomplete_details/reason")
        .and_then(|v| v.as_str());
    if reason == Some("max_output_tokens") {
        return true;
    }
    if is_type("response.completed") {
        let status = value.pointer("/response/status").and_then(|v| v.as_str());
        return status.is_some_and(|s| s != "completed");
    }
    false
}

/// Extract text from an SSE event whose `event:` is `response.output_text.delta`
/// (or whose JSON `type` field matches). Data may be split across multiple `data:` lines.
/// `response.usage.{input_tokens,output_tokens}` from a Responses API
/// `response.completed` (or `response.incomplete`) event; `None` otherwise.
pub fn parse_sse_response_usage(event_block: &str) -> Option<TokenUsage> {
    let (event_name, value) = parse_sse_event(event_block)?;
    let json_type = value.get("type").and_then(|v| v.as_str());
    let kind = event_name.as_deref().or(json_type)?;
    if kind != "response.completed" && kind != "response.incomplete" {
        return None;
    }
    let u = value.pointer("/response/usage")?;
    if u.is_null() {
        return None;
    }
    Some(TokenUsage {
        input: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        output: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

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
    let resp = send_with_retry(builder, "list models").await?;
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
    let resp = send_with_retry(builder, what).await?;
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

    /// The shared client is built once; every call hands back a usable clone.
    #[tokio::test]
    async fn http_client_is_shared_and_reusable() {
        let first = http_client().expect("first client");
        let second = http_client().expect("second client");
        let (base, served) = spawn_http_sequence(vec![(200, "{}"), (200, "{}")]);
        let a = first.get(format!("{base}/a")).send().await.unwrap();
        assert_eq!(a.status().as_u16(), 200);
        let b = second.get(format!("{base}/b")).send().await.unwrap();
        assert_eq!(b.status().as_u16(), 200);
        assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn retry_delay_uses_short_hint_or_jittered_base() {
        assert_eq!(
            retry_delay(Some(Duration::from_secs(3))),
            Duration::from_secs(3)
        );
        for _ in 0..8 {
            let d = retry_delay(None);
            assert!(d >= RETRY_BASE_DELAY, "{d:?}");
            assert!(
                d < RETRY_BASE_DELAY + Duration::from_millis(RETRY_JITTER_MS),
                "{d:?}"
            );
        }
    }

    #[tokio::test]
    async fn retries_once_after_503_then_succeeds() {
        let (base, served) =
            spawn_http_sequence(vec![(503, r#"{"error":"busy"}"#), (200, OPENAI_OK)]);
        let out = openai_provider(&base, "gpt-4o-mini")
            .complete(simple_req())
            .await
            .unwrap();
        assert_eq!(out, "ok");
        assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retries_429_honouring_retry_after_zero() {
        let started = std::time::Instant::now();
        let (base, served) = spawn_http_sequence_with_headers(vec![
            (429, "Retry-After: 0\r\n", r#"{"error":"rate limited"}"#),
            (200, "", OPENAI_OK),
        ]);
        let out = openai_provider(&base, "gpt-4o-mini")
            .complete(simple_req())
            .await
            .unwrap();
        assert_eq!(out, "ok");
        assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(
            started.elapsed() < RETRY_BASE_DELAY,
            "Retry-After: 0 should skip the default backoff, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn does_not_retry_500() {
        let (base, served) =
            spawn_http_sequence(vec![(500, r#"{"error":"boom"}"#), (200, OPENAI_OK)]);
        let err = openai_provider(&base, "gpt-4o-mini")
            .complete(simple_req())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("500"), "{msg}");
        assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn gives_up_after_second_503() {
        let (base, served) = spawn_http_sequence(vec![
            (503, r#"{"error":"busy"}"#),
            (503, r#"{"error":"still busy"}"#),
            (200, OPENAI_OK),
        ]);
        let err = openai_provider(&base, "gpt-4o-mini")
            .complete(simple_req())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("503"), "{msg}");
        assert!(
            msg.contains("still busy"),
            "second attempt's body expected: {msg}"
        );
        assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn list_models_retries_transient_status() {
        let models = r#"{"data":[{"id":"gpt-4o-mini"}]}"#;
        let (base, served) =
            spawn_http_sequence(vec![(504, "<html>timeout</html>"), (200, models)]);
        let ids = list_provider_models(ProviderKind::OpenAiCompatible, &base, "k")
            .await
            .unwrap();
        assert_eq!(ids, vec!["gpt-4o-mini"]);
        assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 2);
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
        use std::io::Write;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = body.to_string();
        let content_type = content_type.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request_body = read_http_request(&mut stream);
            let _ = tx.send(request_body);
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        });
        (format!("http://{addr}"), rx)
    }

    /// Read one HTTP/1.1 request (headers, then `Content-Length` bytes of
    /// body) from the stream and return the body.
    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        use std::io::Read;
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
        let Some(end) = header_end else {
            return String::new();
        };
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

    /// Fake HTTP server that answers N sequential connections, one canned
    /// JSON response each, in order. Returns the base URL and a counter of
    /// requests actually served, so tests can assert how many attempts the
    /// retry logic made. Every response closes its connection.
    fn spawn_http_sequence(
        responses: Vec<(u16, &str)>,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        spawn_http_sequence_with_headers(
            responses
                .into_iter()
                .map(|(status, body)| (status, "", body))
                .collect(),
        )
    }

    /// Like [`spawn_http_sequence`] with extra raw header lines per response
    /// (each terminated by `\r\n`, e.g. `"Retry-After: 0\r\n"`).
    fn spawn_http_sequence_with_headers(
        responses: Vec<(u16, &str, &str)>,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::Write;
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let responses: Vec<(u16, String, String)> = responses
            .into_iter()
            .map(|(s, h, b)| (s, h.to_string(), b.to_string()))
            .collect();
        let served = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&served);
        std::thread::spawn(move || {
            for (status, extra_headers, body) in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let _ = read_http_request(&mut stream);
                counter.fetch_add(1, Ordering::SeqCst);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (format!("http://{addr}"), served)
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

    /// The usage store is process-wide and tests run in parallel: hold this
    /// while a test sets a store path, and use a unique model id per test so
    /// concurrent recordings from other tests never match its assertions.
    static USAGE_STORE_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct UsageStore {
        path: std::path::PathBuf,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl UsageStore {
        fn new(tag: &str) -> Self {
            let guard = USAGE_STORE_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let path = std::env::temp_dir().join(format!(
                "selara-usage-test-{}-{tag}.jsonl",
                std::process::id()
            ));
            let _ = std::fs::remove_file(&path);
            usage::set_store(Some(path.clone()));
            Self {
                path,
                _guard: guard,
            }
        }

        fn model_totals(&self, model: &str) -> Option<(u64, u64, u64)> {
            let s = usage::summary(&self.path).unwrap();
            s.models
                .iter()
                .find(|m| m.model == model)
                .map(|m| (m.totals.requests, m.totals.input, m.totals.output))
        }
    }

    impl Drop for UsageStore {
        fn drop(&mut self) {
            usage::set_store(None);
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[tokio::test]
    async fn openai_records_usage_from_buffered_reply() {
        let store = UsageStore::new("openai-buffered");
        let payload = r#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}}"#;
        let base = spawn_http(200, payload, "application/json");
        openai_provider(&base, "usage-test-openai-buffered")
            .complete(simple_req())
            .await
            .unwrap();
        assert_eq!(
            store.model_totals("usage-test-openai-buffered"),
            Some((1, 12, 3))
        );
        let s = usage::summary(&store.path).unwrap();
        let m = &s.models[0];
        assert_eq!(m.kind, usage::KIND_OPENAI_COMPATIBLE);
        assert_eq!(m.totals.cost_usd, None, "unknown model has no cost");
    }

    #[tokio::test]
    async fn openai_missing_usage_is_skipped() {
        let store = UsageStore::new("openai-no-usage");
        let base = spawn_http(200, OPENAI_OK, "application/json");
        openai_provider(&base, "usage-test-openai-none")
            .complete(simple_req())
            .await
            .unwrap();
        assert_eq!(store.model_totals("usage-test-openai-none"), None);
    }

    #[tokio::test]
    async fn openai_stream_records_final_usage_chunk() {
        let store = UsageStore::new("openai-stream");
        let d1 = openai_delta("\"Hello\"");
        let fin = openai_finish("stop");
        let usage_chunk =
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":20,\"completion_tokens\":5}}\n\n";
        let (base, rx) = spawn_http_chunked(
            200,
            "text/event-stream",
            vec![&d1, &fin, usage_chunk, "data: [DONE]\n\n"],
        );
        let out = openai_provider(&base, "usage-test-openai-stream")
            .complete_stream(simple_req(), &mut |_: &str| {})
            .await
            .unwrap();
        assert_eq!(out, "Hello");
        let sent: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(sent["stream_options"]["include_usage"], true, "{sent}");
        assert_eq!(
            store.model_totals("usage-test-openai-stream"),
            Some((1, 20, 5))
        );
    }

    #[tokio::test]
    async fn anthropic_records_usage_from_buffered_reply() {
        let store = UsageStore::new("anthropic-buffered");
        let payload = r#"{"content":[{"type":"text","text":"claude-ok"}],"stop_reason":"end_turn","usage":{"input_tokens":30,"output_tokens":7}}"#;
        let base = spawn_http(200, payload, "application/json");
        AnthropicProvider {
            api_key: "k".into(),
            model: "usage-test-anthropic-buffered".into(),
            base_url: base,
        }
        .complete(simple_req())
        .await
        .unwrap();
        assert_eq!(
            store.model_totals("usage-test-anthropic-buffered"),
            Some((1, 30, 7))
        );
        let s = usage::summary(&store.path).unwrap();
        assert_eq!(s.models[0].kind, usage::KIND_ANTHROPIC);
    }

    #[tokio::test]
    async fn anthropic_stream_records_start_and_delta_usage() {
        let store = UsageStore::new("anthropic-stream");
        let start = anthropic_event(
            "message_start",
            r#"{"type":"message_start","message":{"id":"m1","role":"assistant","content":[],"usage":{"input_tokens":41,"output_tokens":1}}}"#,
        );
        let d1 = anthropic_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
        );
        let msg_delta = anthropic_event(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}"#,
        );
        let stop = anthropic_event("message_stop", r#"{"type":"message_stop"}"#);
        let (base, _rx) = spawn_http_chunked(
            200,
            "text/event-stream",
            vec![&start, &d1, &msg_delta, &stop],
        );
        let mut provider = anthropic_provider(base);
        provider.model = "usage-test-anthropic-stream".into();
        let out = provider
            .complete_stream(simple_req(), &mut |_: &str| {})
            .await
            .unwrap();
        assert_eq!(out, "hi");
        assert_eq!(
            store.model_totals("usage-test-anthropic-stream"),
            Some((1, 41, 9))
        );
    }

    #[test]
    fn codex_usage_parses_response_completed_only() {
        let done = "event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":100,\"output_tokens\":25,\"total_tokens\":125}}}\n";
        assert_eq!(
            parse_sse_response_usage(done),
            Some(TokenUsage {
                input: 100,
                output: 25
            })
        );
        let delta = "event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n";
        assert_eq!(parse_sse_response_usage(delta), None);
        let no_usage =
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n";
        assert_eq!(parse_sse_response_usage(no_usage), None);
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
            model: "m".into(),
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

    #[test]
    fn anthropic_max_tokens_clamps_to_input_size() {
        assert_eq!(anthropic_max_tokens(0), 4096);
        assert_eq!(anthropic_max_tokens(100), 4096);
        assert_eq!(anthropic_max_tokens(8192), 4096);
        assert_eq!(anthropic_max_tokens(20_000), 10_000);
        assert_eq!(anthropic_max_tokens(1_000_000), 32_000);
    }

    #[test]
    fn sse_truncation_detects_incomplete_terminal_events() {
        let incomplete = "event: response.incomplete\n\
data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n";
        assert!(sse_event_marks_truncation(incomplete));

        let completed_but_cut = "event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n";
        assert!(sse_event_marks_truncation(completed_but_cut));

        let completed_wrong_status =
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"incomplete\"}}\n";
        assert!(sse_event_marks_truncation(completed_wrong_status));
    }

    #[test]
    fn sse_truncation_ignores_normal_events() {
        let completed = "event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"incomplete_details\":null}}\n";
        assert!(!sse_event_marks_truncation(completed));

        let delta = "event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n";
        assert!(!sse_event_marks_truncation(delta));

        assert!(!sse_event_marks_truncation("data: [DONE]\n"));
        assert!(!sse_event_marks_truncation("event: ping\n"));
    }

    #[tokio::test]
    async fn html_error_page_keeps_http_status() {
        // 502 is retried once, so serve it twice: the error shape must
        // survive retry exhaustion.
        let (base, served) = spawn_http_sequence(vec![
            (502, "<html>bad gateway</html>"),
            (502, "<html>bad gateway</html>"),
        ]);
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
        assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 2);
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
    /// Fake HTTP server that answers one request with a `Transfer-Encoding:
    /// chunked` body, one HTTP chunk per entry in `chunks` with a short pause
    /// between them, so SSE framing across network reads is exercised. Returns
    /// the base URL and the captured request body.
    fn spawn_http_chunked(
        status: u16,
        content_type: &str,
        chunks: Vec<&str>,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::Write;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let content_type = content_type.to_string();
        let chunks: Vec<String> = chunks.into_iter().map(str::to_string).collect();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request_body = read_http_request(&mut stream);
            let _ = tx.send(request_body);
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
            for chunk in chunks {
                let framed = format!("{:x}\r\n{chunk}\r\n", chunk.len());
                let _ = stream.write_all(framed.as_bytes());
                let _ = stream.flush();
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        });
        (format!("http://{addr}"), rx)
    }

    fn openai_delta(content: &str) -> String {
        format!("data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{content}}},\"finish_reason\":null}}]}}\n\n")
    }

    fn openai_finish(reason: &str) -> String {
        format!(
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"{reason}\"}}]}}\n\n"
        )
    }

    #[tokio::test]
    async fn openai_stream_yields_deltas_in_order() {
        let role = "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":null},\"finish_reason\":null}]}\n\n";
        let d1 = openai_delta("\"Hel\"");
        let d2 = openai_delta("\"lo, \"");
        let d3 = openai_delta("\"world\"");
        let fin = openai_finish("stop");
        let (base, rx) = spawn_http_chunked(
            200,
            "text/event-stream",
            vec![role, &d1, &d2, &d3, &fin, "data: [DONE]\n\n"],
        );
        let mut seen: Vec<String> = Vec::new();
        let out = openai_provider(&base, "gpt-4o-mini")
            .complete_stream(simple_req(), &mut |d: &str| seen.push(d.to_string()))
            .await
            .unwrap();
        assert_eq!(out, "Hello, world");
        assert_eq!(seen, vec!["Hel", "lo, ", "world"]);
        let sent: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(sent["stream"], true, "{sent}");
        assert_eq!(sent["temperature"], 0.2, "{sent}");
    }

    #[tokio::test]
    async fn openai_stream_finish_reason_length_is_an_error_after_draining() {
        let d1 = openai_delta("\"partial\"");
        let fin = openai_finish("length");
        let (base, _rx) = spawn_http_chunked(
            200,
            "text/event-stream",
            vec![&d1, &fin, "data: [DONE]\n\n"],
        );
        let mut seen = 0usize;
        let err = openai_provider(&base, "gpt-4o-mini")
            .complete_stream(simple_req(), &mut |_: &str| seen += 1)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cut off"), "{msg}");
        assert!(msg.contains("finish_reason=length"), "{msg}");
        assert!(
            !msg.contains("partial"),
            "must not leak partial output: {msg}"
        );
        assert_eq!(seen, 1, "the delta before the cut-off is still forwarded");
    }

    #[tokio::test]
    async fn openai_stream_http_error_keeps_status_and_body() {
        let base = spawn_http(
            401,
            r#"{"error":{"message":"bad key"}}"#,
            "application/json",
        );
        let err = openai_provider(&base, "gpt-4o-mini")
            .complete_stream(simple_req(), &mut |_: &str| {})
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("401") && msg.contains("bad key"), "{msg}");
    }

    /// A server that ignores `stream: true` and answers with one JSON body is
    /// still usable: the reply is delivered as a single fragment.
    #[tokio::test]
    async fn openai_stream_accepts_buffered_json_reply() {
        let base = spawn_http(200, OPENAI_OK, "application/json");
        let mut seen: Vec<String> = Vec::new();
        let out = openai_provider(&base, "gpt-4o-mini")
            .complete_stream(simple_req(), &mut |d: &str| seen.push(d.to_string()))
            .await
            .unwrap();
        assert_eq!(out, "ok");
        assert_eq!(seen, vec!["ok"]);
    }

    fn anthropic_event(name: &str, data: &str) -> String {
        format!("event: {name}\ndata: {data}\n\n")
    }

    fn anthropic_provider(base: String) -> AnthropicProvider {
        AnthropicProvider {
            api_key: "k".into(),
            model: "m".into(),
            base_url: base,
        }
    }

    #[tokio::test]
    async fn anthropic_stream_yields_text_deltas() {
        let start = anthropic_event(
            "message_start",
            r#"{"type":"message_start","message":{"id":"m1","role":"assistant","content":[]}}"#,
        );
        let block_start = anthropic_event(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        );
        let d1 = anthropic_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"claude"}}"#,
        );
        let d2 = anthropic_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"-ok"}}"#,
        );
        let block_stop = anthropic_event(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        );
        let msg_delta = anthropic_event(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#,
        );
        let stop = anthropic_event("message_stop", r#"{"type":"message_stop"}"#);
        let (base, rx) = spawn_http_chunked(
            200,
            "text/event-stream",
            vec![
                &start,
                &block_start,
                &d1,
                &d2,
                &block_stop,
                &msg_delta,
                &stop,
            ],
        );
        let mut seen: Vec<String> = Vec::new();
        let out = anthropic_provider(base)
            .complete_stream(simple_req(), &mut |d: &str| seen.push(d.to_string()))
            .await
            .unwrap();
        assert_eq!(out, "claude-ok");
        assert_eq!(seen, vec!["claude", "-ok"]);
        let sent: serde_json::Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(sent["stream"], true, "{sent}");
        assert_eq!(sent["max_tokens"], 4096, "{sent}");
    }

    #[tokio::test]
    async fn anthropic_stream_max_tokens_is_an_error() {
        let d1 = anthropic_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}"#,
        );
        let msg_delta = anthropic_event(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"}}"#,
        );
        let stop = anthropic_event("message_stop", r#"{"type":"message_stop"}"#);
        let (base, _rx) =
            spawn_http_chunked(200, "text/event-stream", vec![&d1, &msg_delta, &stop]);
        let err = anthropic_provider(base)
            .complete_stream(simple_req(), &mut |_: &str| {})
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cut off"), "{msg}");
        assert!(msg.contains("stop_reason=max_tokens"), "{msg}");
    }

    #[tokio::test]
    async fn anthropic_stream_error_event_is_an_error() {
        let start = anthropic_event(
            "message_start",
            r#"{"type":"message_start","message":{"id":"m1"}}"#,
        );
        let error = anthropic_event(
            "error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        );
        let (base, _rx) = spawn_http_chunked(200, "text/event-stream", vec![&start, &error]);
        let err = anthropic_provider(base)
            .complete_stream(simple_req(), &mut |_: &str| {})
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Overloaded"), "{msg}");
    }

    /// A provider that only implements `complete` still streams: the default
    /// `complete_stream` delivers the whole reply as one fragment.
    #[tokio::test]
    async fn default_complete_stream_delivers_one_fragment() {
        struct Buffered;
        #[async_trait]
        impl LlmProvider for Buffered {
            async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
                Ok(format!("echo:{}", req.user))
            }
        }
        let mut seen: Vec<String> = Vec::new();
        let out = Buffered
            .complete_stream(simple_req(), &mut |d: &str| seen.push(d.to_string()))
            .await
            .unwrap();
        assert_eq!(out, "echo:u");
        assert_eq!(seen, vec!["echo:u"]);
    }

    #[test]
    fn drain_utf8_waits_for_a_split_multibyte_character() {
        // "é" is 0xC3 0xA9; deliver the two bytes in separate chunks.
        let mut pending = vec![b'a', 0xC3];
        let mut out = String::new();
        drain_utf8(&mut pending, &mut out);
        assert_eq!(out, "a");
        assert_eq!(pending, vec![0xC3]);
        pending.push(0xA9);
        pending.push(b'b');
        drain_utf8(&mut pending, &mut out);
        assert_eq!(out, "a\u{e9}b");
        assert!(pending.is_empty());
        // A truly invalid byte is decoded lossily rather than held forever.
        let mut bad = vec![0xFF, b'x'];
        let mut out = String::new();
        drain_utf8(&mut bad, &mut out);
        assert_eq!(out, "\u{fffd}x");
        assert!(bad.is_empty());
    }
}
