//! Local usage ledger: token counts per request, kept on this machine only.
//!
//! Providers call [`record`] with whatever token counts the API reported.
//! Events go to an in-memory list and, once [`set_store`] has been given a
//! path, are appended as one JSON line each to `usage.jsonl` next to the
//! config file (owner-only). Nothing here talks to the network; the numbers
//! never leave the machine. [`summary`] aggregates the file into per-model
//! totals plus `today` / `last_30_days` / `all_time` buckets with an
//! estimated cost from [`price_per_million`] where the model is known.

use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Tokens billed for one request, as the provider reported them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
}

/// One recorded request. `ts` is Unix seconds; `kind` is the provider label
/// (`openai_compatible`, `openrouter`, `anthropic`, `chatgpt_codex`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageEvent {
    pub ts: u64,
    pub kind: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
}

/// Totals for one time window (or one model).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageBucket {
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    /// Estimated USD from the built-in price table, summed over the events
    /// whose model has a known price. `None` when no event could be priced.
    pub cost_usd: Option<f64>,
    /// Requests that carried no known price (local or unlisted models).
    pub unpriced: u64,
}

/// Per `(kind, model)` totals, all time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsage {
    pub kind: String,
    pub model: String,
    #[serde(flatten)]
    pub totals: UsageBucket,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub path: String,
    pub today: UsageBucket,
    pub last_30_days: UsageBucket,
    pub all_time: UsageBucket,
    pub models: Vec<ModelUsage>,
}

/// Provider labels used in the ledger.
pub const KIND_OPENAI_COMPATIBLE: &str = "openai_compatible";
pub const KIND_OPENROUTER: &str = "openrouter";
pub const KIND_ANTHROPIC: &str = "anthropic";
pub const KIND_CHATGPT_CODEX: &str = "chatgpt_codex";

/// Newest events kept in memory (oldest are dropped past this).
const IN_MEMORY_CAP: usize = 10_000;
const DAY_SECS: u64 = 86_400;

static EVENTS: Mutex<Vec<UsageEvent>> = Mutex::new(Vec::new());
static STORE: Mutex<Option<PathBuf>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `usage.jsonl` next to the config file.
pub fn usage_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name("usage.jsonl")
}

/// Set (or unset) the file every later [`record`] appends to.
pub fn set_store(path: Option<PathBuf>) {
    *lock(&STORE) = path;
}

/// Current store path, if any.
pub fn store() -> Option<PathBuf> {
    lock(&STORE).clone()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record one request. Appends to memory and, when a store is set, to the
/// ledger file. Write failures are logged and otherwise ignored: usage
/// accounting must never fail a completion.
pub fn record(kind: &str, model: &str, usage: TokenUsage) {
    let event = UsageEvent {
        ts: now_secs(),
        kind: kind.to_string(),
        model: model.to_string(),
        input: usage.input,
        output: usage.output,
    };
    {
        let mut events = lock(&EVENTS);
        if events.len() >= IN_MEMORY_CAP {
            let excess = events.len() + 1 - IN_MEMORY_CAP;
            events.drain(..excess);
        }
        events.push(event.clone());
    }
    if let Some(path) = store() {
        if let Err(e) = append_event(&path, &event) {
            tracing::warn!("usage: could not append to {}: {e}", path.display());
        }
    }
}

/// Events recorded by this process, oldest first.
pub fn in_memory() -> Vec<UsageEvent> {
    lock(&EVENTS).clone()
}

fn append_event(path: &Path, event: &UsageEvent) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    let mut line = serde_json::to_string(event).map_err(std::io::Error::other)?;
    line.push('\n');
    file.write_all(line.as_bytes())
}

/// Truncate the ledger file (a missing file is fine).
pub fn clear(path: &Path) -> Result<(), CoreError> {
    if !path.exists() {
        return Ok(());
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?;
    Ok(())
}

/// Read every parseable event in the ledger. Malformed lines are skipped so
/// one bad write never hides the rest.
pub fn read_events(path: &Path) -> Result<Vec<UsageEvent>, CoreError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<UsageEvent>(line.trim()).ok())
        .collect())
}

/// Aggregate the ledger at `path` as of now.
pub fn summary(path: &Path) -> Result<UsageSummary, CoreError> {
    let events = read_events(path)?;
    Ok(summarize(path, &events, now_secs()))
}

/// Aggregate `events` as of `now` (Unix seconds). "Today" is the UTC day
/// containing `now`; "last 30 days" is the trailing 30 × 24 h window.
pub fn summarize(path: &Path, events: &[UsageEvent], now: u64) -> UsageSummary {
    let day_start = now - now % DAY_SECS;
    let month_start = now.saturating_sub(30 * DAY_SECS);
    let mut out = UsageSummary {
        path: path.display().to_string(),
        ..Default::default()
    };
    let mut per_model: BTreeMap<(String, String), UsageBucket> = BTreeMap::new();
    for e in events {
        let cost = event_cost(e);
        out.all_time.add(e, cost);
        if e.ts >= month_start {
            out.last_30_days.add(e, cost);
        }
        if e.ts >= day_start {
            out.today.add(e, cost);
        }
        per_model
            .entry((e.kind.clone(), e.model.clone()))
            .or_default()
            .add(e, cost);
    }
    out.models = per_model
        .into_iter()
        .map(|((kind, model), totals)| ModelUsage {
            kind,
            model,
            totals,
        })
        .collect();
    out
}

impl UsageBucket {
    fn add(&mut self, e: &UsageEvent, cost: Option<f64>) {
        self.requests += 1;
        self.input += e.input;
        self.output += e.output;
        match cost {
            Some(c) => self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + c),
            None => self.unpriced += 1,
        }
    }
}

/// Estimated USD for one event, if the model is priced.
pub fn event_cost(e: &UsageEvent) -> Option<f64> {
    let (input, output) = price_per_million(&e.model)?;
    Some((e.input as f64 * input + e.output as f64 * output) / 1_000_000.0)
}

/// Approximate list prices in USD per million tokens `(input, output)`.
///
/// These are estimates from public price pages at the time of writing, not a
/// billing source of truth: providers change prices, cached and batch tiers
/// differ, and OpenRouter adds its own margin. Treat the resulting cost as a
/// rough guide; a user-editable override table is a planned follow-up. Local
/// and unlisted models return `None` (no cost shown). A provider prefix such
/// as `openai/` or `anthropic/` is ignored, and dated or suffixed variants
/// (`gpt-4o-2024-08-06`, `claude-sonnet-5-20260201`) match their base id.
pub fn price_per_million(model: &str) -> Option<(f64, f64)> {
    // Most specific prefixes first so `gpt-4.1-mini` is not priced as `gpt-4.1`.
    const TABLE: &[(&str, f64, f64)] = &[
        ("gpt-4.1-nano", 0.10, 0.40),
        ("gpt-4.1-mini", 0.40, 1.60),
        ("gpt-4.1", 2.00, 8.00),
        ("gpt-4o-mini", 0.15, 0.60),
        ("gpt-4o", 2.50, 10.00),
        ("o4-mini", 1.10, 4.40),
        ("claude-opus-5", 5.00, 25.00),
        ("claude-sonnet-5", 3.00, 15.00),
        ("claude-haiku-4-5", 1.00, 5.00),
        ("claude-haiku-4.5", 1.00, 5.00),
    ];
    let id = model.trim().to_ascii_lowercase();
    let id = id.rsplit('/').next().unwrap_or(&id);
    TABLE
        .iter()
        .find(|(prefix, _, _)| {
            id == *prefix
                || id
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('-') || rest.starts_with(':'))
        })
        .map(|(_, i, o)| (*i, *o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts: u64, model: &str, input: u64, output: u64) -> UsageEvent {
        UsageEvent {
            ts,
            kind: "openai_compatible".into(),
            model: model.into(),
            input,
            output,
        }
    }

    #[test]
    fn price_lookup_matches_known_ids_and_variants() {
        assert_eq!(price_per_million("gpt-4o-mini"), Some((0.15, 0.60)));
        assert_eq!(price_per_million("openai/gpt-4o-mini"), Some((0.15, 0.60)));
        assert_eq!(price_per_million("GPT-4o-2024-08-06"), Some((2.50, 10.00)));
        assert_eq!(price_per_million("gpt-4.1-mini"), Some((0.40, 1.60)));
        assert_eq!(price_per_million("gpt-4.1"), Some((2.00, 8.00)));
        assert_eq!(price_per_million("o4-mini"), Some((1.10, 4.40)));
        assert_eq!(
            price_per_million("claude-sonnet-5-20260201"),
            Some((3.00, 15.00))
        );
        assert_eq!(price_per_million("claude-opus-5"), Some((5.00, 25.00)));
        assert_eq!(price_per_million("claude-haiku-4-5"), Some((1.00, 5.00)));
    }

    #[test]
    fn price_lookup_skips_local_and_unknown_models() {
        for id in ["llama3.1:8b", "gpt-4ox", "gpt-4", "", "mistral"] {
            assert_eq!(price_per_million(id), None, "{id}");
        }
    }

    #[test]
    fn summary_buckets_by_window_and_model() {
        let now = 1_800_000_000; // some UTC instant
        let day_start = now - now % DAY_SECS;
        let events = vec![
            ev(now - 10, "gpt-4o-mini", 1_000_000, 1_000_000), // today: $0.75
            ev(day_start.saturating_sub(1), "gpt-4o-mini", 100, 10), // yesterday
            ev(now - 40 * DAY_SECS, "llama3.1:8b", 5, 5),      // older than 30d, unpriced
            ev(now - 2 * DAY_SECS, "llama3.1:8b", 7, 3),       // this month, unpriced
        ];
        let s = summarize(Path::new("/x/usage.jsonl"), &events, now);
        assert_eq!(s.path, "/x/usage.jsonl");

        assert_eq!(s.today.requests, 1);
        assert_eq!(s.today.input, 1_000_000);
        assert_eq!(s.today.output, 1_000_000);
        assert!((s.today.cost_usd.unwrap() - 0.75).abs() < 1e-9);
        assert_eq!(s.today.unpriced, 0);

        assert_eq!(s.last_30_days.requests, 3);
        assert_eq!(s.last_30_days.input, 1_000_107);
        assert_eq!(s.last_30_days.unpriced, 1);

        assert_eq!(s.all_time.requests, 4);
        assert_eq!(s.all_time.output, 1_000_018);
        assert_eq!(s.all_time.unpriced, 2);

        assert_eq!(s.models.len(), 2);
        let mini = s.models.iter().find(|m| m.model == "gpt-4o-mini").unwrap();
        assert_eq!(mini.totals.requests, 2);
        assert!(mini.totals.cost_usd.is_some());
        let llama = s.models.iter().find(|m| m.model == "llama3.1:8b").unwrap();
        assert_eq!(llama.totals.cost_usd, None);
        assert_eq!(llama.totals.unpriced, 2);
    }

    #[test]
    fn empty_ledger_has_no_cost() {
        let s = summarize(Path::new("/x"), &[], 1_800_000_000);
        assert_eq!(s.all_time, UsageBucket::default());
        assert!(s.models.is_empty());
    }

    #[test]
    fn ledger_round_trips_and_skips_bad_lines() {
        let dir = std::env::temp_dir().join(format!("selara-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("usage.jsonl");
        let _ = std::fs::remove_file(&path);
        append_event(&path, &ev(1, "gpt-4o", 3, 4)).unwrap();
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            f.write_all(b"not json\n").unwrap();
        }
        append_event(&path, &ev(2, "gpt-4o", 5, 6)).unwrap();
        let events = read_events(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].input, 5);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "ledger must be owner-only, got {mode:o}");
        }
        clear(&path).unwrap();
        assert!(read_events(&path).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            read_events(&path).unwrap().is_empty(),
            "missing file is empty"
        );
    }

    #[test]
    fn usage_path_sits_next_to_config() {
        assert_eq!(
            usage_path(Path::new("/a/b/config.toml")),
            PathBuf::from("/a/b/usage.jsonl")
        );
    }
}
