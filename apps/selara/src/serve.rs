//! macOS writing service: capture a selection, transform it, and paste once.
use std::collections::VecDeque;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use eframe::egui;
use notify::Watcher;
use selara_core::commands::{
    command_applies_to, run_command_stream, CommandKind, PromptVars, WritingCommand,
};
use selara_core::config::{app_is_excluded, serve_pidfile, AppConfig, ProviderAuth};
use selara_core::desktop_protocol::{
    AxTrust, ProtocolCommand, ProtocolDecoder, ProtocolRequest, ProtocolResponse, ServeReadiness,
    ServeStatus, PROTOCOL_VERSION,
};
use selara_core::guard::{provider_is_hosted, scan_secrets, SecretHit, SecretKind};
use selara_core::history::{self, HistoryEntry};
use selara_platform::macos::{
    accessibility_trusted, activate_pid, frontmost_app_name, frontmost_bundle_id, frontmost_pid,
    mouse_location, prompt_accessibility, screen_visible_frame_at, HotkeyAction, MacosHotkey,
    MacosSelection,
};
use selara_platform::SelectionService;

const DIALOG_SIZE: (f32, f32) = (400.0, 280.0);
const CURSOR_OFFSET: f64 = 12.0;

#[derive(Debug)]
enum JobResult {
    Delta { generation: u64, text: String },
    Success { generation: u64, text: String },
    Error { generation: u64, message: String },
}
impl JobResult {
    fn generation(&self) -> u64 {
        match self {
            Self::Delta { generation, .. }
            | Self::Success { generation, .. }
            | Self::Error { generation, .. } => *generation,
        }
    }
}
fn should_apply_job(job: u64, current: u64, waiting: bool) -> bool {
    waiting && job == current
}
fn can_cancel(run: u64, current: u64, active: bool) -> bool {
    active && run == current
}
fn selected_text(text: Option<String>) -> Option<String> {
    text.filter(|s| !s.trim().is_empty())
}
/// Top-left corner for a window of `size` opened next to `cursor`: 12 pt right
/// and below it, clamped so the whole window stays inside `visible`
/// `(x, y, w, h)`. A window larger than the frame sits at the frame's origin so
/// its top-left (filter box, first rows) is always reachable.
fn place_near(cursor: (f64, f64), size: (f64, f64), visible: (f64, f64, f64, f64)) -> (f64, f64) {
    fn clamp_axis(want: f64, origin: f64, extent: f64, len: f64) -> f64 {
        let max = origin + extent - len;
        if max < origin {
            origin
        } else {
            want.clamp(origin, max)
        }
    }
    let (vx, vy, vw, vh) = visible;
    (
        clamp_axis(cursor.0 + CURSOR_OFFSET, vx, vw, size.0),
        clamp_axis(cursor.1 + CURSOR_OFFSET, vy, vh, size.1),
    )
}

/// Picker banner for a selection that looks like it holds secrets. Names each
/// kind once (first preview only) and never the full value.
fn secret_banner(hits: &[SecretHit]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut seen: Vec<SecretKind> = Vec::new();
    for hit in hits {
        if seen.contains(&hit.kind) {
            continue;
        }
        seen.push(hit.kind);
        parts.push(format!("{} ({})", hit.kind.with_article(), hit.preview));
    }
    let list = match parts.len() {
        0 => "a secret".to_string(),
        1 => parts[0].clone(),
        n => format!("{} and {}", parts[..n - 1].join(", "), parts[n - 1]),
    };
    format!("Looks like it contains {list}. This goes to a hosted provider. Send anyway?")
}

/// Id of the command built from free-form text typed into the instruction dialog. It is
/// never written to the config: "Save as command" derives a real id first.
const ADHOC_ID: &str = "adhoc";

/// Where the command that is running (or just ran) came from.
///
/// This is tracked alongside the command rather than inferred from its id.
/// Command ids are free-form — a hand-edited `config.toml` or an imported
/// command pack can perfectly well contain `id = "adhoc"` — so a predicate on
/// the id would file that configured command's prompt into instruction history
/// and offer "Save as command…" for something already saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandOrigin {
    /// A command that already exists in the config.
    Configured,
    /// One-off text typed into the instruction dialog; in memory only.
    Instruction,
}

/// How many instructions the instruction dialog remembers for ↑ recall.
const HISTORY_CAP: usize = 10;

/// A one-off command from the text in the instruction dialog. It runs through
/// the same guarded execution path as a configured command, but only lives in
/// memory unless the user saves it from the instruction dialog.
fn adhoc_command(text: &str) -> WritingCommand {
    WritingCommand {
        id: ADHOC_ID.into(),
        label: "Instruction".into(),
        kind: CommandKind::Replace,
        prompt: text.trim().to_string(),
        hotkey: None,
        model: None,
        apps: Vec::new(),
    }
}

/// Label for a saved instruction: its first four words.
fn instruction_label(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().take(4).collect();
    if words.is_empty() {
        "Instruction".into()
    } else {
        words.join(" ")
    }
}

/// `Make it shorter, please!` → `make-it-shorter-please`, the same rule as the
/// Settings app's `slug`: ASCII alphanumerics kept and lowercased, every other
/// run collapsed to one dash, no leading or trailing dash, `command` when
/// nothing is left. Capped at 32 chars so a long label still gives a short id.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in text.chars() {
        if out.len() >= 32 {
            break;
        }
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "command".into()
    } else {
        out
    }
}

/// `slug` plus a five-hex-digit tail taken from `seed`, re-derived until the
/// id is not in `existing`. Deterministic for a given seed so it can be
/// tested; the caller feeds it the clock, mirroring the random tail the
/// Settings app appends so two similar instructions never collide.
fn unique_command_id(slug: &str, existing: &[String], mut seed: u64) -> String {
    loop {
        let id = format!("{slug}-{:05x}", seed % 0x10_0000);
        if !existing.contains(&id) {
            return id;
        }
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
    }
}

/// Turn an instruction into a saved command: label
/// from its first words, id from their slug plus a tail unique among
/// `existing`, replacement behavior, and no shortcut or model override.
fn command_from_instruction(text: &str, kind: CommandKind, existing: &[String]) -> WritingCommand {
    let prompt = text.trim().to_string();
    let label = instruction_label(&prompt);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    WritingCommand {
        id: unique_command_id(&slugify(&label), existing, seed),
        label,
        kind,
        prompt,
        hotkey: None,
        model: None,
        apps: Vec::new(),
    }
}

/// Remember `text` as the most recent instruction. A repeat moves to the
/// front instead of appearing twice; only the last `HISTORY_CAP` are kept.
fn push_history(history: &mut VecDeque<String>, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    history.retain(|h| h != text);
    history.push_front(text.to_string());
    history.truncate(HISTORY_CAP);
}

/// Where ↑ (`delta > 0`, older) or ↓ (`delta < 0`, newer) lands while walking
/// a history of `len` entries stored newest first. `None` is the empty box:
/// ↑ from there recalls the newest entry and ↓ from the newest returns to it.
/// Walking past the oldest entry stays on it.
fn history_step(current: Option<usize>, len: usize, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match (current, delta.signum()) {
        (None, 1) => Some(0),
        (Some(i), 1) => Some((i + 1).min(len - 1)),
        (Some(0), -1) => None,
        (Some(i), -1) => Some(i - 1),
        (current, _) => current,
    }
}

/// `1234567` → `1,234,567`, for the streaming progress counter.
fn format_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Progress line shown while a Replace command streams. The text itself is
/// held back until it is complete and cleaned, so only its size is shown.
fn replace_progress(partial: &str) -> String {
    format!(
        "… {} chars so far",
        format_thousands(partial.chars().count())
    )
}

enum UiPhase {
    Hidden,
    Instruction,
    Confirm,
    Working { label: String, partial: String },
    Error { message: String },
}
impl UiPhase {
    fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Instruction | Self::Confirm | Self::Working { .. }
        )
    }
}
#[derive(Default)]
struct WorkGate {
    quiescing: bool,
    paused: bool,
    active: usize,
}
impl WorkGate {
    fn admit(&mut self) -> bool {
        if self.quiescing {
            return false;
        }
        self.active += 1;
        true
    }
    fn finish(&mut self) {
        self.active = self.active.saturating_sub(1);
    }
    fn quiesce(&mut self) {
        self.quiescing = true;
    }
    fn pause_if_idle(&mut self) -> bool {
        if self.quiescing && self.active == 0 {
            self.paused = true;
            true
        } else {
            false
        }
    }
    fn resume(&mut self) {
        self.quiescing = false;
        self.paused = false;
    }
}

enum ProtocolInput {
    Frame(Result<ProtocolRequest, String>),
    Closed,
}

struct ServeApp {
    config: AppConfig,
    request_config: AppConfig,
    config_path: PathBuf,
    pidfile: PathBuf,
    selection: Arc<MacosSelection>,
    hotkey: MacosHotkey,
    config_mtime: Option<SystemTime>,
    reload_error: Option<String>,
    config_dirty: Arc<AtomicBool>,
    _config_watcher: Option<notify::RecommendedWatcher>,
    last_config_poll: Instant,
    egui_ctx: egui::Context,
    phase: UiPhase,
    progress: crate::progress::ProgressPanel,
    captured_text: String,
    captured_app: Option<String>,
    captured_range: Option<(i64, i64)>,
    target_pid: Option<i32>,
    soft_warn_acked: bool,
    secret_hits: Vec<SecretHit>,
    secret_guard_acked: bool,
    replace_warn_acked: bool,
    pending_direct: Option<(WritingCommand, CommandOrigin)>,
    generation: u64,
    last_command: Option<WritingCommand>,
    job_rx: Receiver<JobResult>,
    job_tx: Sender<JobResult>,
    runtime: tokio::runtime::Runtime,
    instruction: String,
    focus_instruction: bool,
    instruction_history: VecDeque<String>,
    history_cursor: Option<usize>,
    notice: String,
    protocol: Option<Receiver<ProtocolInput>>,
    gate: WorkGate,
    pending_quiesce: Vec<String>,
    last_protocol_status: Option<ServeStatus>,
    last_protocol_emit: Instant,
    protocol_output: Option<Arc<std::sync::Mutex<std::io::BufWriter<std::io::Stdout>>>>,
}

impl ServeApp {
    fn protocol_status(&self) -> ServeStatus {
        ServeStatus {
            version: PROTOCOL_VERSION,
            id: None,
            readiness: if self.gate.paused {
                ServeReadiness::Quiesced
            } else if self.gate.quiescing {
                ServeReadiness::Quiescing
            } else if self.gate.active > 0 || self.phase.is_active() {
                ServeReadiness::Busy
            } else {
                ServeReadiness::Ready
            },
            ax_trust: if accessibility_trusted() {
                AxTrust::Granted
            } else {
                AxTrust::Missing
            },
            generation: self.generation,
            external: false,
        }
    }

    fn protocol_write(
        output: &Arc<std::sync::Mutex<std::io::BufWriter<std::io::Stdout>>>,
        response: &ProtocolResponse,
    ) {
        if let Ok(mut out) = output.lock() {
            if serde_json::to_writer(&mut *out, response).is_ok() {
                let _ = out.write_all(b"\n");
                let _ = out.flush();
            }
        }
    }

    fn new(
        cc: &eframe::CreationContext<'_>,
        config: AppConfig,
        config_path: PathBuf,
        selection: Arc<MacosSelection>,
    ) -> Result<Self> {
        let hotkey = MacosHotkey::new();
        let egui_ctx = cc.egui_ctx.clone();
        let wake = egui_ctx.clone();
        hotkey.set_wake(move || wake.request_repaint());
        Self::register_hotkeys(&hotkey, &config)?;
        let config_mtime = std::fs::metadata(&config_path)
            .and_then(|m| m.modified())
            .ok();
        let config_dirty = Arc::new(AtomicBool::new(false));
        let watcher = Self::watch_config(&config_path, &config_dirty, &egui_ctx);
        let (job_tx, job_rx) = mpsc::channel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            request_config: config.clone(),
            config,
            pidfile: serve_pidfile(&config_path),
            config_path,
            selection,
            hotkey,
            config_mtime,
            reload_error: None,
            config_dirty,
            _config_watcher: watcher,
            last_config_poll: Instant::now(),
            egui_ctx,
            phase: UiPhase::Hidden,
            progress: crate::progress::ProgressPanel::new(),
            captured_text: String::new(),
            captured_app: None,
            captured_range: None,
            target_pid: None,
            soft_warn_acked: false,
            secret_hits: Vec::new(),
            secret_guard_acked: false,
            replace_warn_acked: false,
            pending_direct: None,
            generation: 0,
            last_command: None,
            job_rx,
            job_tx,
            runtime,
            instruction: String::new(),
            focus_instruction: false,
            instruction_history: VecDeque::new(),
            history_cursor: None,
            notice: String::new(),
            protocol: None,
            gate: WorkGate::default(),
            pending_quiesce: Vec::new(),
            last_protocol_status: None,
            last_protocol_emit: Instant::now(),
            protocol_output: None,
        })
    }
    /// Watch the config's directory (not the file: atomic saves rename a new
    /// inode into place) and flag a reload plus a repaint on any change. Returns
    /// `None` when the watcher cannot be created; the periodic poll still runs.
    fn watch_config(
        config_path: &std::path::Path,
        dirty: &Arc<AtomicBool>,
        egui_ctx: &egui::Context,
    ) -> Option<notify::RecommendedWatcher> {
        let dir = config_path.parent()?.to_path_buf();
        let dirty = dirty.clone();
        let ctx = egui_ctx.clone();
        let mut watcher =
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
                Ok(_) => {
                    dirty.store(true, Ordering::SeqCst);
                    ctx.request_repaint();
                }
                Err(e) => tracing::warn!("config watcher error: {e}"),
            }) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!("config watcher unavailable ({e}); polling every 5s instead");
                    return None;
                }
            };
        if let Err(e) = watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
            tracing::warn!(
                "cannot watch {} ({e}); polling every 5s instead",
                dir.display()
            );
            return None;
        }
        tracing::info!("selara: watching {} for config changes", dir.display());
        Some(watcher)
    }

    fn start_protocol(&mut self) {
        let (send, receive) = mpsc::sync_channel(128);
        self.protocol = Some(receive);
        self.protocol_output = Some(Arc::new(std::sync::Mutex::new(std::io::BufWriter::new(
            std::io::stdout(),
        ))));
        let wake = self.egui_ctx.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut decoder = ProtocolDecoder::default();
            let mut bytes = [0; 8192];
            loop {
                match stdin.read(&mut bytes) {
                    Ok(0) | Err(_) => {
                        let _ = send.send(ProtocolInput::Closed);
                        wake.request_repaint();
                        break;
                    }
                    Ok(count) => {
                        for frame in decoder.push(&bytes[..count]) {
                            if send.send(ProtocolInput::Frame(frame)).is_err() {
                                return;
                            }
                            wake.request_repaint();
                        }
                    }
                }
            }
        });
    }

    fn poll_protocol(&mut self, ctx: &egui::Context) {
        let frames: Vec<_> = self
            .protocol
            .as_ref()
            .map(|r| r.try_iter().collect())
            .unwrap_or_default();
        for frame in frames {
            let request = match frame {
                ProtocolInput::Closed => {
                    self.gate.quiesce();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    continue;
                }
                ProtocolInput::Frame(Err(error)) => {
                    if let Some(output) = &self.protocol_output {
                        Self::protocol_write(
                            output,
                            &ProtocolResponse::error(None, self.protocol_status(), error),
                        );
                    }
                    continue;
                }
                ProtocolInput::Frame(Ok(request)) => request,
            };
            let mut error = None;
            if request.version != PROTOCOL_VERSION {
                error = Some("Unsupported desktop protocol version".to_string());
            } else {
                match request.command {
                    ProtocolCommand::RunCommand {
                        command_id,
                        target_pid,
                    } => {
                        error = self
                            .begin_run(ctx, Some(&command_id), Some(target_pid))
                            .err();
                        if let Some(message) = &error {
                            self.fail(ctx, message.clone());
                        }
                    }
                    ProtocolCommand::CustomInstruction { target_pid } => {
                        error = self.begin_run(ctx, None, Some(target_pid)).err();
                        if let Some(message) = &error {
                            self.fail(ctx, message.clone());
                        }
                    }
                    ProtocolCommand::Cancel { run_id } => {
                        if can_cancel(run_id, self.generation, self.phase.is_active()) {
                            self.hide(ctx);
                        } else {
                            error = Some("That command is no longer active".into());
                        }
                    }
                    ProtocolCommand::Status => {}
                    ProtocolCommand::RequestPermission => prompt_accessibility(),
                    ProtocolCommand::Quiesce => {
                        self.gate.quiesce();
                        self.hide(ctx);
                        self.pending_quiesce.push(request.id);
                        continue;
                    }
                    ProtocolCommand::Resume => {
                        // Cancel pending barriers before reopening admissions. Their
                        // old acknowledgement must never satisfy a later request.
                        let ids = std::mem::take(&mut self.pending_quiesce);
                        for id in ids {
                            if let Some(output) = &self.protocol_output {
                                Self::protocol_write(
                                    output,
                                    &ProtocolResponse::error(
                                        Some(id),
                                        self.protocol_status(),
                                        "Pause cancelled",
                                    ),
                                );
                            }
                        }
                        self.gate.resume();
                    }
                    ProtocolCommand::ReloadAuth => {
                        if self.gate.active != 0 {
                            error = Some("Finish active work before changing accounts".into());
                        } else if let Err(e) =
                            self.runtime.block_on(selara_core::app_server::reset())
                        {
                            error = Some(e.to_string());
                        }
                    }
                }
            }
            if let Some(output) = &self.protocol_output {
                let response = match error {
                    Some(error) => {
                        ProtocolResponse::error(Some(request.id), self.protocol_status(), error)
                    }
                    None => ProtocolResponse::ok(Some(request.id), self.protocol_status()),
                };
                Self::protocol_write(output, &response);
            }
        }
    }

    fn finish_protocol_frame(&mut self, ctx: &egui::Context) {
        if self.protocol.is_none() {
            return;
        }
        if !self.pending_quiesce.is_empty() && self.gate.pause_if_idle() {
            // Job results have been drained on this same UI thread. Dismiss
            // queued confirmations and invalidate every late write.
            self.hide(ctx);
            let ids = std::mem::take(&mut self.pending_quiesce);
            for id in ids {
                if let Some(output) = &self.protocol_output {
                    Self::protocol_write(
                        output,
                        &ProtocolResponse::ok(Some(id), self.protocol_status()),
                    );
                }
            }
        }
        let status = self.protocol_status();
        if self.last_protocol_status.as_ref() != Some(&status)
            || self.last_protocol_emit.elapsed() >= Duration::from_secs(5)
        {
            if let Some(output) = &self.protocol_output {
                Self::protocol_write(output, &ProtocolResponse::ok(None, status.clone()));
            }
            self.last_protocol_status = Some(status);
            self.last_protocol_emit = Instant::now();
        }
        // Also catches permission changes while a dialog is idle.
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn position_near_cursor(&self, ctx: &egui::Context) -> Option<egui::Pos2> {
        let cursor = mouse_location()?;
        let visible = screen_visible_frame_at(cursor.0, cursor.1)?;
        // No OS decorations, so the outer size is the inner size. Prefer the
        // live size in case the user resized the window.
        let size = ctx
            .input(|i| {
                i.viewport()
                    .outer_rect
                    .map(|r| (f64::from(r.width()), f64::from(r.height())))
            })
            .unwrap_or((f64::from(DIALOG_SIZE.0), f64::from(DIALOG_SIZE.1)));
        let (x, y) = place_near(cursor, size, visible);
        Some(egui::pos2(x as f32, y as f32))
    }

    fn register_hotkeys(hotkey: &MacosHotkey, config: &AppConfig) -> Result<()> {
        let cmd_keys: Vec<(String, String)> = config
            .commands
            .iter()
            .filter_map(|c| {
                c.hotkey
                    .as_ref()
                    .map(|h| h.trim().to_string())
                    .filter(|h| !h.is_empty())
                    .map(|h| (c.id.clone(), h))
            })
            .collect();
        tracing::info!(
            "selara: registering custom instruction `{}` + {} command shortcut(s)",
            config.hotkey,
            cmd_keys.len()
        );
        for (id, spec) in &cmd_keys {
            tracing::info!("selara:   command `{id}` → `{spec}`");
        }
        hotkey.reregister_all(&config.hotkey, &cmd_keys)?;
        Ok(())
    }

    /// Reload when the watcher flagged a change, or every 5 s as a fallback.
    fn poll_config(&mut self) {
        let dirty = self.config_dirty.swap(false, Ordering::SeqCst);
        if !dirty && self.last_config_poll.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.last_config_poll = Instant::now();
        self.maybe_reload_config();
    }

    fn maybe_reload_config(&mut self) {
        let Ok(meta) = std::fs::metadata(&self.config_path) else {
            return;
        };
        let Ok(mtime) = meta.modified() else {
            return;
        };
        if self.config_mtime == Some(mtime) {
            return;
        }
        // Keep an admitted request and its frozen config intact when a live
        // config edit is invalid. Report the reload error once the run ends.
        self.config_mtime = Some(mtime);
        match AppConfig::load_or_init(&self.config_path) {
            Ok(cfg) => match Self::register_hotkeys(&self.hotkey, &cfg) {
                Ok(()) => {
                    self.config = cfg;
                    self.reload_error = None;
                }
                Err(e) => self.reload_error = Some(format!("Hotkey reload failed: {e}")),
            },
            Err(e) => self.reload_error = Some(format!("Config reload failed: {e}")),
        }
    }

    fn selection_chars(&self) -> u64 {
        self.captured_text.chars().count() as u64
    }

    fn over_hard_max(&self) -> bool {
        let max = self.request_config.limits.hard_max_chars;
        max > 0 && self.selection_chars() > max
    }

    fn needs_soft_warn(&self) -> bool {
        let soft = self.request_config.limits.soft_warn_chars;
        soft > 0 && self.selection_chars() > soft && !self.soft_warn_acked
    }

    fn needs_replace_warn(&self) -> bool {
        let warn = self.request_config.limits.replace_warn_chars;
        warn > 0 && self.selection_chars() > warn && !self.replace_warn_acked
    }

    /// Requests leave the machine: the ChatGPT/Codex path always does, and a
    /// BYOK provider does unless its base URL points at a local server.
    fn provider_hosted(&self) -> bool {
        let p = &self.request_config.provider;
        matches!(p.auth, ProviderAuth::ChatGpt) || provider_is_hosted(p.kind, &p.base_url)
    }

    fn needs_secret_warn(&self) -> bool {
        self.request_config.limits.secret_guard
            && !self.secret_hits.is_empty()
            && !self.secret_guard_acked
            && self.provider_hosted()
    }

    fn record_history(&self, result: &str) {
        let Some(cmd) = self.last_command.as_ref() else {
            return;
        };
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = HistoryEntry {
            ts,
            command_id: cmd.id.clone(),
            label: cmd.label.clone(),
            kind: CommandKind::Replace,
            app: self.captured_app.clone(),
            original: self.captured_text.clone(),
            result: result.to_string(),
        };
        let path = history::history_path(&self.config_path);
        if let Err(e) = history::append(&path, &entry) {
            tracing::warn!("history: could not append to {}: {e}", path.display());
        }
    }

    fn show_window(&self, ctx: &egui::Context, focus: bool) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        if focus {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::AlwaysOnTop,
        ));
    }

    fn show_dialog(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            DIALOG_SIZE.0,
            DIALOG_SIZE.1,
        )));
        if let Some(pos) = self.position_near_cursor(ctx) {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
        self.show_window(ctx, true);
    }

    fn fail(&mut self, ctx: &egui::Context, message: String) {
        let _ = self.hotkey.set_cancel_enabled(false);
        self.progress.hide();
        self.pending_direct = None;
        self.phase = UiPhase::Error { message };
        self.show_dialog(ctx);
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.generation += 1;
        self.progress.hide();
        self.pending_direct = None;
        self.phase = UiPhase::Hidden;
        let _ = self.hotkey.set_cancel_enabled(false);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    /// Capture before any Selara window becomes key. A tray request must still
    /// refer to the foreground source app; a stale PID is never reactivated here.
    fn begin_run(
        &mut self,
        ctx: &egui::Context,
        command_id: Option<&str>,
        target: Option<i32>,
    ) -> Result<(), String> {
        if self.gate.quiescing {
            return Err("The writing service is paused".into());
        }
        let target = target
            .or_else(frontmost_pid)
            .ok_or("Select text first in another app")?;
        if target == std::process::id() as i32 || frontmost_pid() != Some(target) {
            tracing::warn!(requested_pid = target, frontmost_pid = ?frontmost_pid(), "ignoring command for a stale source app");
            return Err("Select text first in the source app, then run the command again".into());
        }
        let name = frontmost_app_name();
        let bundle = frontmost_bundle_id();
        if app_is_excluded(
            &self.config.excluded_apps,
            name.as_deref(),
            bundle.as_deref(),
        ) {
            // Excluded apps must not cause a selection read or show a window.
            return Ok(());
        }
        let command = command_id
            .map(|id| {
                self.config
                    .commands
                    .iter()
                    .find(|c| c.id == id)
                    .cloned()
                    .ok_or_else(|| format!("Unknown command `{id}`. Refresh the command menu."))
            })
            .transpose()?;
        if let Some(cmd) = &command {
            if !command_applies_to(cmd, name.as_deref(), bundle.as_deref()) {
                return Err(format!(
                    "{} is not enabled for this app. Edit its Apps setting to change that.",
                    cmd.label
                ));
            }
        }
        if !accessibility_trusted() {
            prompt_accessibility();
            return Err("Enable Accessibility for Selara in System Settings → Privacy & Security → Accessibility.".into());
        }
        self.hide(ctx);
        self.target_pid = Some(target);
        self.request_config = self.config.clone();
        self.soft_warn_acked = false;
        self.secret_guard_acked = false;
        self.replace_warn_acked = false;
        self.instruction.clear();
        self.notice.clear();
        self.history_cursor = None;
        let snapshot = self
            .runtime
            .block_on(self.selection.read_selection())
            .map_err(|e| e.to_string())?;
        let snapshot = snapshot.ok_or("Select text first, then run the command again")?;
        self.captured_text = selected_text(Some(snapshot.text))
            .ok_or("Select text first, then run the command again")?;
        self.captured_app = snapshot.app_name;
        self.captured_range = snapshot.range;
        self.secret_hits = if self.request_config.limits.secret_guard {
            scan_secrets(&self.captured_text)
        } else {
            Vec::new()
        };
        if self.over_hard_max() {
            return Err(format!("Selection is {} characters, over your hard limit of {}. Select less text or change Limits in Settings.", self.selection_chars(), self.request_config.limits.hard_max_chars));
        }
        match command {
            Some(cmd) => self.prepare_command(ctx, cmd, CommandOrigin::Configured),
            None => {
                self.phase = UiPhase::Instruction;
                self.focus_instruction = true;
                self.show_dialog(ctx);
                Ok(())
            }
        }
    }

    fn prepare_command(
        &mut self,
        ctx: &egui::Context,
        mut command: WritingCommand,
        origin: CommandOrigin,
    ) -> Result<(), String> {
        command.kind = CommandKind::Replace;
        if self.needs_soft_warn() || self.needs_replace_warn() || self.needs_secret_warn() {
            self.pending_direct = Some((command, origin));
            self.phase = UiPhase::Confirm;
            self.show_dialog(ctx);
            Ok(())
        } else {
            self.start_command(ctx, command, origin)
        }
    }

    /// Only return from our own dialog to the captured app. Do not steal focus
    /// back from an app the user switched to while an instruction was open.
    fn return_to_source(&self, ctx: &egui::Context) -> Result<(), String> {
        let pid = self
            .target_pid
            .ok_or("The source app is no longer available")?;
        match frontmost_pid() {
            Some(current) if current == pid => Ok(()),
            Some(current) if current == std::process::id() as i32 => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                activate_pid(pid)
                    .map_err(|e| format!("Could not return to the source app: {e}"))?;
                if frontmost_pid() != Some(pid) {
                    return Err("The source app could not receive focus".into());
                }
                Ok(())
            }
            _ => Err("The focused app changed. Select text and run the command again.".into()),
        }
    }

    fn start_command(
        &mut self,
        ctx: &egui::Context,
        command: WritingCommand,
        origin: CommandOrigin,
    ) -> Result<(), String> {
        if self.gate.quiescing {
            return Err("The writing service is paused".into());
        }
        self.return_to_source(ctx)?;
        // Revalidate before transmitting too: dismissing a dialog must not
        // turn a stale selection into an expensive request.
        self.selection
            .validate_captured_selection(self.target_pid, &self.captured_text, self.captured_range)
            .map_err(|e| e.to_string())?;
        if !self.gate.admit() {
            return Err("The writing service is paused".into());
        }
        self.pending_direct = None;
        if origin == CommandOrigin::Instruction {
            push_history(&mut self.instruction_history, &command.prompt);
        }
        self.last_command = Some(command.clone());
        self.phase = UiPhase::Working {
            label: command.label.clone(),
            partial: String::new(),
        };
        if let Err(e) = self.hotkey.set_cancel_enabled(true) {
            tracing::warn!("Escape could not be registered; use Cancel: {e}");
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        self.progress.show(&command.label);
        let cfg = self.request_config.clone();
        let input = self.captured_text.clone();
        let app_name = self.captured_app.clone();
        let generation = self.generation;
        let tx = self.job_tx.clone();
        let wake = ctx.clone();
        self.runtime.spawn(async move {
            let result = async {
                let provider = cfg.build_provider_for(&command)?;
                let delta_tx = tx.clone();
                let delta_wake = wake.clone();
                let mut on_delta = move |text: &str| {
                    let _ = delta_tx.send(JobResult::Delta {
                        generation,
                        text: text.into(),
                    });
                    delta_wake.request_repaint();
                };
                run_command_stream(
                    provider.as_ref(),
                    &command,
                    &input,
                    None,
                    PromptVars {
                        language: Some(&cfg.language),
                        app: app_name.as_deref(),
                    },
                    &mut on_delta,
                )
                .await
            }
            .await;
            let job = match result {
                Ok(text) => JobResult::Success { generation, text },
                Err(e) => JobResult::Error {
                    generation,
                    message: e.to_string(),
                },
            };
            let _ = tx.send(job);
            wake.request_repaint();
        });
        Ok(())
    }

    fn apply_job(&mut self, ctx: &egui::Context, job: JobResult) {
        match job {
            JobResult::Delta { text, .. } => {
                if let UiPhase::Working { label, partial } = &mut self.phase {
                    partial.push_str(&text);
                    self.progress.update(label, partial.chars().count());
                }
            }
            JobResult::Error { message, .. } => self.fail(ctx, message),
            JobResult::Success { text, .. } => {
                let result = if text.trim().is_empty() {
                    Err(anyhow::anyhow!(
                        "The provider returned no text. Your selection was not changed."
                    ))
                } else {
                    self.selection.replace_in_app(
                        self.target_pid,
                        &text,
                        &self.captured_text,
                        self.captured_range,
                    )
                };
                match result {
                    Ok(()) => { self.record_history(&text); self.hide(ctx); }
                    Err(e) => self.fail(ctx, format!("Replacement could not be verified: {e}\nNo automatic retry was attempted.")),
                }
            }
        }
    }

    fn save_instruction(&mut self) {
        let prompt = self.instruction.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        let mut label = String::new();
        let result = AppConfig::update(&self.config_path, |latest| {
            let ids = latest
                .commands
                .iter()
                .map(|c| c.id.clone())
                .collect::<Vec<_>>();
            let command = command_from_instruction(&prompt, CommandKind::Replace, &ids);
            label = command.label.clone();
            latest.commands.push(command);
            Ok(())
        });
        self.notice = match result {
            Ok(_) => format!("Saved as “{label}”. Edit it in Settings → Commands."),
            Err(e) => format!("Could not save command: {e}"),
        };
    }
}
impl eframe::App for ServeApp {
    fn on_exit(&mut self) {
        let _ = self.hotkey.set_cancel_enabled(false);
        let _ = self.runtime.block_on(selara_core::app_server::reset());
        remove_pidfile(&self.pidfile);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if matches!(self.phase, UiPhase::Hidden | UiPhase::Working { .. }) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if self.progress.take_cancelled() {
            self.hide(ctx);
        }
        self.hotkey.poll();
        self.poll_config();
        self.poll_protocol(ctx);
        if let Some(action) = self.hotkey.take_pending() {
            if !self.gate.quiescing {
                let result = match action {
                    HotkeyAction::CustomInstruction => self.begin_run(ctx, None, None),
                    HotkeyAction::Command(id) => self.begin_run(ctx, Some(&id), None),
                    HotkeyAction::Cancel => {
                        self.hide(ctx);
                        Ok(())
                    }
                };
                if let Err(message) = result {
                    self.fail(ctx, message);
                }
            }
        }
        while let Ok(job) = self.job_rx.try_recv() {
            if !matches!(&job, JobResult::Delta { .. }) {
                self.gate.finish();
            }
            let waiting = matches!(self.phase, UiPhase::Working { .. });
            if !self.gate.quiescing && should_apply_job(job.generation(), self.generation, waiting)
            {
                self.apply_job(ctx, job);
            }
        }
        if !self.gate.quiescing && matches!(self.phase, UiPhase::Hidden) {
            if let Some(message) = self.reload_error.take() {
                self.fail(ctx, message);
            }
        }
        self.finish_protocol_frame(ctx);
        ctx.request_repaint_after(if matches!(self.phase, UiPhase::Working { .. }) {
            Duration::from_millis(50)
        } else {
            Duration::from_secs(1)
        });
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.gate.quiescing || matches!(self.phase, UiPhase::Hidden) {
            return;
        }
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.hide(&ctx);
            return;
        }
        let mut run = false;
        let mut save = false;
        let mut confirm = false;
        let mut dismiss = false;
        match &self.phase {
            UiPhase::Instruction => {
                ui.heading("Custom instruction");
                ui.label(format!(
                    "Transform the selection in {}.",
                    self.captured_app.as_deref().unwrap_or("the source app")
                ));
                let recall = ctx.input(|i| {
                    if i.key_pressed(egui::Key::ArrowUp)
                        && (self.instruction.is_empty() || self.history_cursor.is_some())
                    {
                        1
                    } else if i.key_pressed(egui::Key::ArrowDown) && self.history_cursor.is_some() {
                        -1
                    } else {
                        0
                    }
                });
                if recall != 0 {
                    self.history_cursor =
                        history_step(self.history_cursor, self.instruction_history.len(), recall);
                    self.instruction = self
                        .history_cursor
                        .and_then(|i| self.instruction_history.get(i))
                        .cloned()
                        .unwrap_or_default();
                }
                let edit = ui.add(
                    egui::TextEdit::multiline(&mut self.instruction)
                        .hint_text("What should change?")
                        .desired_rows(4),
                );
                if self.focus_instruction {
                    edit.request_focus();
                    self.focus_instruction = false;
                }
                if edit.changed() {
                    self.history_cursor = None;
                }
                if !self.notice.is_empty() {
                    ui.small(&self.notice);
                }
                let has_text = !self.instruction.trim().is_empty();
                ui.horizontal(|ui| {
                    run = ui
                        .add_enabled(has_text, egui::Button::new("Replace selection"))
                        .clicked();
                    save = ui
                        .add_enabled(has_text, egui::Button::new("Save as command…"))
                        .clicked();
                    dismiss = ui.button("Cancel").clicked();
                });
                run |= has_text
                    && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
                ui.small("⌘Enter to run · ↑ recalls recent instructions");
            }
            UiPhase::Confirm => {
                ui.heading("Confirm replacement");
                if self.needs_secret_warn() {
                    ui.label(secret_banner(&self.secret_hits));
                }
                if self.needs_soft_warn() {
                    ui.label(format!(
                        "This sends {} characters to your provider.",
                        self.selection_chars()
                    ));
                }
                if self.needs_replace_warn() {
                    ui.label(format!(
                        "This replaces {} selected characters. Use the source app’s ⌘Z to undo.",
                        self.selection_chars()
                    ));
                }
                ui.horizontal(|ui| {
                    confirm = ui.button("Continue").clicked();
                    dismiss = ui.button("Cancel").clicked();
                });
            }
            UiPhase::Working { label, partial } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.heading(label);
                });
                ui.label(replace_progress(partial));
                ui.small("Your selection will be replaced when ready.");
                dismiss = ui.button("Cancel (Esc)").clicked();
            }
            UiPhase::Error { message } => {
                ui.heading("Selara");
                ui.label(message);
                dismiss = ui.button("Close").clicked();
            }
            UiPhase::Hidden => {}
        }
        if dismiss {
            self.hide(&ctx);
            return;
        }
        if save {
            self.save_instruction();
        }
        let result = if run {
            self.prepare_command(
                &ctx,
                adhoc_command(&self.instruction),
                CommandOrigin::Instruction,
            )
        } else if confirm {
            self.soft_warn_acked = true;
            self.secret_guard_acked = true;
            self.replace_warn_acked = true;
            if let Some((command, origin)) = self.pending_direct.take() {
                self.start_command(&ctx, command, origin)
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };
        if let Err(message) = result {
            self.fail(&ctx, message);
        }
    }
}

/// `kill(pid, 0)` succeeds only while a process with that id exists and is
/// ours to signal; anything else means the pidfile is stale.
fn pid_alive(pid: i32) -> bool {
    pid > 0 && unsafe { libc::kill(pid, 0) == 0 }
}

/// Record our pid in `serve.pid` (owner-only) so the Settings app's Status tab
/// can tell whether the shell is running.
///
/// A pidfile naming a live process means another shell already owns the global
/// hotkeys, so the second start is refused instead of overwriting it. Taking the
/// file over would put a pid in it that dies the moment hotkey registration
/// fails — the Status tab would then report "Not running" while the original
/// shell is perfectly healthy — and would let whichever process exits first
/// delete the other's file. A leftover file from a crashed run names no live
/// process and is replaced.
fn write_pidfile(path: &Path) -> Result<()> {
    if let Ok(raw) = std::fs::read_to_string(path) {
        if let Ok(old) = raw.trim().parse::<i32>() {
            if old != std::process::id() as i32 && pid_alive(old) {
                anyhow::bail!(
                    "selara serve is already running (pid {old}, recorded in {}). \
                     Stop that process first; if it is gone, delete the file.",
                    path.display()
                );
            }
        }
        tracing::info!("selara: removing stale pidfile {}", path.display());
        let _ = std::fs::remove_file(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("create pidfile {}", path.display()))?;
    writeln!(file, "{}", std::process::id())?;
    tracing::info!(
        "selara: pid {} recorded in {}",
        std::process::id(),
        path.display()
    );
    Ok(())
}

/// Remove `serve.pid`, but only while it still names this process.
///
/// The counterpart to `write_pidfile` refusing a live owner: if the file has
/// since been taken over (say the user deleted it by hand and started another
/// shell), deleting it on our way out would make the running shell invisible to
/// the Status tab.
fn remove_pidfile(path: &Path) {
    if let Ok(raw) = std::fs::read_to_string(path) {
        let ours = std::process::id() as i32;
        if let Ok(recorded) = raw.trim().parse::<i32>() {
            if recorded != ours {
                tracing::info!(
                    "selara: {} names pid {recorded}, not ours ({ours}); leaving it in place",
                    path.display()
                );
                return;
            }
        }
    }
    match std::fs::remove_file(path) {
        Ok(()) => tracing::info!("selara: removed pidfile {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("selara: could not remove {}: {e}", path.display()),
    }
}

pub fn run(config_path: PathBuf, desktop_protocol: bool) -> Result<()> {
    let config = AppConfig::load_or_init(&config_path)?;
    write_pidfile(&serve_pidfile(&config_path))?;
    selara_core::usage::set_store(Some(selara_core::usage::usage_path(&config_path)));
    if !desktop_protocol {
        println!("config: {}", config_path.display());
    }
    if !desktop_protocol {
        println!("hotkey: {}", config.hotkey);
    }
    let cmd_shortcuts: Vec<String> = config
        .commands
        .iter()
        .filter_map(|c| {
            c.hotkey
                .as_ref()
                .map(|h| h.trim())
                .filter(|h| !h.is_empty())
                .map(|h| format!("{} ({})", c.label, h))
        })
        .collect();
    if desktop_protocol {
    } else if cmd_shortcuts.is_empty() {
        println!("command shortcuts: (none)");
    } else {
        println!("command shortcuts: {}", cmd_shortcuts.join(", "));
    }
    if !desktop_protocol {
        println!(
            "limits: soft_warn={} hard_max={} replace_warn={} secret_guard={}",
            config.limits.soft_warn_chars,
            config.limits.hard_max_chars,
            config.limits.replace_warn_chars,
            if config.limits.secret_guard {
                "on"
            } else {
                "off"
            }
        );
    }
    if !desktop_protocol {
        println!(
            "accessibility: {}",
            if accessibility_trusted() {
                "granted"
            } else {
                "MISSING — grant under System Settings → Privacy & Security → Accessibility"
            }
        );
    }
    if !accessibility_trusted() {
        prompt_accessibility();
    }

    let selection = Arc::new(MacosSelection::new()?);
    let config_path_for_app = config_path.clone();

    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([DIALOG_SIZE.0, DIALOG_SIZE.1])
            .with_min_inner_size([320.0, 120.0])
            .with_resizable(true)
            // Borderless: the header row inside the panel is the drag handle
            // (`ViewportCommand::StartDrag`), and Esc / Close dismiss it.
            .with_decorations(false)
            .with_always_on_top()
            .with_visible(false)
            .with_active(false)
            .with_title("Selara"),
        ..Default::default()
    };
    options.event_loop_builder = Some(Box::new(|builder| {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_activation_policy(winit::platform::macos::ActivationPolicy::Accessory);
    }));

    eframe::run_native(
        "Selara",
        options,
        Box::new(move |cc| {
            Ok(Box::new({
                let mut app = ServeApp::new(cc, config, config_path_for_app, selection)?;
                if desktop_protocol {
                    app.start_protocol();
                }
                app
            }) as Box<dyn eframe::App>)
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{
        adhoc_command, can_cancel, command_from_instruction, format_thousands, history_step,
        instruction_label, place_near, push_history, remove_pidfile, replace_progress,
        secret_banner, selected_text, should_apply_job, slugify, unique_command_id, write_pidfile,
        HISTORY_CAP,
    };
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh directory per call, so the pidfile tests never share a path.
    fn scratch_dir(name: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "selara-pidfile-{}-{name}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// A pid that is guaranteed not to name a live process: pid 0 is never a
    /// valid target for `kill(pid, 0)` in `pid_alive`.
    const DEAD_PID: i32 = 0;

    use selara_core::commands::CommandKind;
    use selara_core::guard::{SecretHit, SecretKind};

    const SIZE: (f64, f64) = (380.0, 440.0);
    /// 1920×1080 display with a 25 pt menu bar and a 70 pt Dock, top-left origin.
    const VISIBLE: (f64, f64, f64, f64) = (0.0, 25.0, 1920.0, 985.0);

    #[test]
    fn place_near_offsets_from_the_cursor_when_there_is_room() {
        assert_eq!(place_near((100.0, 200.0), SIZE, VISIBLE), (112.0, 212.0));
    }

    #[test]
    fn place_near_clamps_at_the_right_and_bottom_edges() {
        // Cursor in the bottom-right corner: the window shifts left and up so
        // it ends exactly at the visible frame's edges (1920 - 380, 1010 - 440).
        assert_eq!(place_near((1900.0, 1000.0), SIZE, VISIBLE), (1540.0, 570.0));
    }

    #[test]
    fn place_near_never_goes_above_the_visible_frame() {
        // Cursor on the menu bar of a secondary display whose visible frame
        // starts at (1920, -200): the window is pushed down onto the frame.
        let secondary = (1920.0, -200.0, 2560.0, 1415.0);
        assert_eq!(
            place_near((2000.0, -230.0), SIZE, secondary),
            (2012.0, -200.0)
        );
    }

    #[test]
    fn place_near_falls_back_to_the_frame_origin_when_the_window_is_larger() {
        let tiny = (100.0, 50.0, 300.0, 200.0);
        assert_eq!(place_near((150.0, 100.0), SIZE, tiny), (100.0, 50.0));
    }

    #[test]
    fn secret_banner_names_each_kind_once_with_a_preview() {
        let hit = |kind, preview: &str| SecretHit {
            kind,
            preview: preview.into(),
        };
        let hits = vec![
            hit(SecretKind::ApiKey, "sk-abc1…"),
            hit(SecretKind::ApiKey, "sk-xyz9…"),
            hit(SecretKind::CardNumber, "4111 1…"),
        ];
        assert_eq!(
            secret_banner(&hits),
            "Looks like it contains an API key (sk-abc1…) and a card number (4111 1…). This goes to a hosted provider. Send anyway?"
        );
        let one = vec![hit(SecretKind::PrivateKey, "-----B…")];
        assert_eq!(
            secret_banner(&one),
            "Looks like it contains a private key (-----B…). This goes to a hosted provider. Send anyway?"
        );
        let three = vec![
            hit(SecretKind::Jwt, "eyJhbG…"),
            hit(SecretKind::ApiKey, "ghp_ab…"),
            hit(SecretKind::CardNumber, "3782 8…"),
        ];
        assert!(secret_banner(&three).starts_with(
            "Looks like it contains a JWT (eyJhbG…), an API key (ghp_ab…) and a card number"
        ));
        // The banner never echoes a full value: previews are what came in.
        assert!(!secret_banner(&hits).contains("sk-abc1234"));
    }

    #[test]
    fn current_generation_while_waiting_is_applied() {
        assert!(should_apply_job(3, 3, true));
    }

    #[test]
    fn stale_generation_is_dropped() {
        // Hotkey pressed again (new capture) while the old request was running.
        assert!(!should_apply_job(3, 4, true));
    }

    #[test]
    fn result_after_escape_is_dropped() {
        // Escape hides the window: phase is no longer Working and the
        // generation was bumped; either condition alone must be enough.
        assert!(!should_apply_job(3, 3, false));
        assert!(!should_apply_job(3, 4, false));
    }

    /// The regression: a second `serve` used to overwrite a live pidfile, so if
    /// its own startup then failed the Status tab reported the healthy original
    /// shell as "Not running".
    #[test]
    fn a_live_pidfile_refuses_the_second_start() {
        let dir = scratch_dir("live");
        let path = dir.join("serve.pid");

        // A real live pid we are allowed to signal, so `pid_alive` says true.
        let mut owner = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn placeholder owner");
        std::fs::write(&path, format!("{}\n", owner.id())).expect("write pidfile");

        let err = write_pidfile(&path).expect_err("a live owner must refuse the start");
        assert!(
            err.to_string().contains("already running"),
            "error names the running shell: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path)
                .expect("pidfile still there")
                .trim(),
            owner.id().to_string(),
            "the original owner's pid is left untouched"
        );

        let _ = owner.kill();
        let _ = owner.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_pidfile_is_replaced() {
        let dir = scratch_dir("stale");
        let path = dir.join("serve.pid");
        std::fs::write(&path, format!("{DEAD_PID}\n")).expect("write stale pidfile");

        write_pidfile(&path).expect("a stale pidfile must not block the start");
        assert_eq!(
            std::fs::read_to_string(&path).expect("pidfile").trim(),
            std::process::id().to_string()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_only_removes_our_own_pidfile() {
        let dir = scratch_dir("cleanup");
        let path = dir.join("serve.pid");

        write_pidfile(&path).expect("write our pidfile");
        remove_pidfile(&path);
        assert!(!path.exists(), "our own pidfile is cleaned up");

        // Someone else's file must survive our exit, or the shell that owns it
        // disappears from the Status tab.
        std::fs::write(&path, "424242\n").expect("write another shell's pidfile");
        remove_pidfile(&path);
        assert_eq!(
            std::fs::read_to_string(&path).expect("pidfile").trim(),
            "424242"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn instruction_label_is_the_first_four_words() {
        assert_eq!(
            instruction_label("Rewrite this as a limerick about cats"),
            "Rewrite this as a"
        );
        assert_eq!(instruction_label("Shorter"), "Shorter");
        assert_eq!(instruction_label("   "), "Instruction");
    }

    #[test]
    fn slugify_matches_the_settings_app() {
        assert_eq!(
            slugify("Make it shorter, please!"),
            "make-it-shorter-please"
        );
        assert_eq!(slugify("  --Rewrite--  "), "rewrite");
        assert_eq!(slugify("¿¡!?"), "command");
        assert_eq!(slugify("Übersetze ins Englische"), "bersetze-ins-englische");
        let long = slugify(&"word ".repeat(20));
        assert!(long.len() <= 32, "{long}");
        assert!(!long.ends_with('-'));
    }

    #[test]
    fn unique_command_id_skips_ids_that_already_exist() {
        let first = unique_command_id("shorter", &[], 42);
        assert!(first.starts_with("shorter-"));
        assert_eq!(first.len(), "shorter-".len() + 5);
        // Same seed, but the first candidate is taken: a different tail.
        let second = unique_command_id("shorter", std::slice::from_ref(&first), 42);
        assert!(second.starts_with("shorter-"));
        assert_ne!(first, second);
        // Deterministic for a seed, so saves are reproducible in tests.
        assert_eq!(first, unique_command_id("shorter", &[], 42));
    }

    #[test]
    fn command_from_instruction_derives_label_id_and_keeps_the_kind() {
        let existing = vec!["proofread".to_string(), "make-it-shorter-1a2b3".to_string()];
        let cmd = command_from_instruction(
            " Make it shorter and punchier ",
            CommandKind::Replace,
            &existing,
        );
        assert_eq!(cmd.label, "Make it shorter and");
        assert_eq!(cmd.prompt, "Make it shorter and punchier");
        assert!(cmd.id.starts_with("make-it-shorter-and-"), "{}", cmd.id);
        assert!(!existing.contains(&cmd.id));
        assert!(matches!(cmd.kind, CommandKind::Replace));
        assert!(cmd.hotkey.is_none() && cmd.model.is_none());
    }

    #[test]
    fn push_history_is_newest_first_deduplicated_and_capped() {
        let mut h = VecDeque::new();
        push_history(&mut h, "a");
        push_history(&mut h, "  ");
        push_history(&mut h, "b");
        push_history(&mut h, " a ");
        assert_eq!(h, VecDeque::from(vec!["a".to_string(), "b".to_string()]));
        for i in 0..20 {
            push_history(&mut h, &format!("n{i}"));
        }
        assert_eq!(h.len(), HISTORY_CAP);
        assert_eq!(h.front().map(String::as_str), Some("n19"));
        assert_eq!(h.back().map(String::as_str), Some("n10"));
    }

    #[test]
    fn history_step_walks_back_from_the_empty_box_and_forward_to_it() {
        assert_eq!(history_step(None, 0, 1), None);
        assert_eq!(history_step(None, 3, 1), Some(0));
        assert_eq!(history_step(Some(0), 3, 1), Some(1));
        assert_eq!(history_step(Some(2), 3, 1), Some(2));
        assert_eq!(history_step(Some(2), 3, -1), Some(1));
        assert_eq!(history_step(Some(0), 3, -1), None);
        assert_eq!(history_step(None, 3, -1), None);
        assert_eq!(history_step(Some(1), 3, 0), Some(1));
    }

    #[test]
    fn thousands_separator_groups_digits() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(1_000), "1,000");
        assert_eq!(format_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn replace_progress_counts_characters_not_bytes() {
        assert_eq!(replace_progress(&"é".repeat(1234)), "… 1,234 chars so far");
    }

    #[test]
    fn selection_is_required_and_kept_verbatim() {
        assert_eq!(selected_text(None), None);
        assert_eq!(selected_text(Some("  ".into())), None);
        assert_eq!(
            selected_text(Some("  café\n".into())),
            Some("  café\n".into())
        );
        assert_eq!(adhoc_command(" Summarize ").kind, CommandKind::Replace);
    }
    #[test]
    fn cancellation_only_matches_the_active_run() {
        assert!(can_cancel(7, 7, true));
        assert!(!can_cancel(6, 7, true));
        assert!(!can_cancel(7, 7, false));
    }
}
#[cfg(test)]
mod work_gate_tests {
    use super::WorkGate;
    #[test]
    fn pause_waits_for_every_worker_even_after_ui_dismissal() {
        let mut gate = WorkGate::default();
        assert!(gate.admit());
        assert!(gate.admit());
        gate.quiesce();
        assert!(!gate.admit());
        assert!(!gate.pause_if_idle());
        gate.finish();
        assert!(!gate.pause_if_idle());
        gate.finish();
        assert!(gate.pause_if_idle());
        assert!(gate.paused);
        assert!(!gate.admit());
        gate.resume();
        assert!(gate.admit());
    }
}
