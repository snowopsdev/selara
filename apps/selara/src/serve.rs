//! macOS desktop shell: global hotkey → command picker → replace / popup.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use notify::Watcher;
use selara_core::commands::{run_command_stream, CommandKind, PromptVars, WritingCommand};
use selara_core::config::{app_is_excluded, serve_pidfile, AppConfig, LimitsConfig};
use selara_platform::macos::{
    accessibility_trusted, activate_pid, frontmost_app_name, frontmost_bundle_id, frontmost_pid,
    mouse_location, prompt_accessibility, screen_visible_frame_at, HotkeyAction, MacosHotkey,
    MacosSelection,
};
use selara_platform::SelectionService;

#[derive(Debug)]
enum JobResult {
    /// One raw text fragment of a reply still in progress; appended to the
    /// `Working` phase's `partial` for display only. The finished text always
    /// arrives separately in `Success`, so a Replace never writes fragments.
    Delta {
        generation: u64,
        text: String,
    },
    Success {
        generation: u64,
        kind: CommandKind,
        label: String,
        text: String,
    },
    Error {
        generation: u64,
        message: String,
    },
}

impl JobResult {
    fn generation(&self) -> u64 {
        match self {
            JobResult::Delta { generation, .. }
            | JobResult::Success { generation, .. }
            | JobResult::Error { generation, .. } => *generation,
        }
    }
}

/// A job result may only be applied when it belongs to the selection that is
/// still current and the UI is still waiting for it. Escape, a new hotkey
/// press, or a dismissed window all bump the generation, so a completion that
/// arrives afterwards is dropped instead of pasted into whatever is focused.
fn should_apply_job(job_generation: u64, current_generation: u64, waiting: bool) -> bool {
    waiting && job_generation == current_generation
}

/// What the last successful Replace (or Insert below) wrote, so it can be put back.
#[derive(Debug, Clone)]
struct LastReplace {
    pid: Option<i32>,
    original: String,
    replacement: String,
    range: Option<(i64, i64)>,
}

/// Which popup actions are clickable for the current selection.
///
/// The replace caution gates both write-back buttons until it is acknowledged;
/// the hard max is never skippable, so the buttons stay enabled and the click
/// explains the block instead (same as `start_command`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PopupActions {
    replace_enabled: bool,
    insert_enabled: bool,
    show_caution: bool,
}

fn popup_actions(over_hard_max: bool, needs_replace_warn: bool) -> PopupActions {
    let blocked_by_caution = needs_replace_warn && !over_hard_max;
    PopupActions {
        replace_enabled: !blocked_by_caution,
        insert_enabled: !blocked_by_caution,
        show_caution: blocked_by_caution,
    }
}

/// Where "Insert below" writes: a zero-length range right after the selection.
fn insert_range((loc, len): (i64, i64)) -> (i64, i64) {
    (loc + len, 0)
}

/// What "Insert below" writes: the result separated from the selection by a blank line.
fn insert_text(body: &str) -> String {
    format!("\n\n{body}")
}

/// Default picker window size in points. Compact enough to sit next to the
/// cursor without covering the text it was opened for.
const PICKER_SIZE: (f32, f32) = (380.0, 440.0);

/// Gap between the cursor and the picker's top-left corner, in points.
const CURSOR_OFFSET: f64 = 12.0;

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

/// Commands whose label contains `query` (case-insensitive, whitespace
/// trimmed). When no label matches, fall back to matching the prompt text so a
/// query like "grammar" still finds Proofread. An empty query returns all.
fn filter_commands<'a>(commands: &'a [WritingCommand], query: &str) -> Vec<&'a WritingCommand> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return commands.iter().collect();
    }
    let by_label: Vec<&WritingCommand> = commands
        .iter()
        .filter(|c| c.label.to_lowercase().contains(&query))
        .collect();
    if !by_label.is_empty() {
        return by_label;
    }
    commands
        .iter()
        .filter(|c| c.prompt.to_lowercase().contains(&query))
        .collect()
}

/// Move the highlighted row by `delta`, wrapping at both ends. `0` for an
/// empty list.
fn next_selection(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as isize;
    ((current as isize + delta).rem_euclid(len)) as usize
}

/// Keep the highlighted row inside the visible list after filtering.
fn clamp_selection(current: usize, len: usize) -> usize {
    current.min(len.saturating_sub(1))
}

/// Whether a picker row may run right now. Mirrors the rails on the buttons:
/// the hard max and an unacknowledged soft warn block everything, and an
/// unacknowledged replace caution blocks Replace commands only.
fn picker_row_enabled(
    is_replace: bool,
    hard_blocked: bool,
    soft_blocked: bool,
    replace_caution: bool,
) -> bool {
    let blocked = hard_blocked || soft_blocked || (is_replace && replace_caution);
    !blocked
}

/// Id of the command built from free-form text typed into the picker. It is
/// never written to the config: "Save as command" derives a real id first.
const ADHOC_ID: &str = "adhoc";

/// How many instructions the picker remembers for ↑ recall.
const HISTORY_CAP: usize = 10;

/// A one-off command from the text in the picker's filter box. It runs through
/// the same pipeline as a configured command (rails, streaming, Retry) but
/// only lives in memory unless the user saves it afterwards.
fn adhoc_command(text: &str, popup: bool) -> WritingCommand {
    WritingCommand {
        id: ADHOC_ID.into(),
        label: "Instruction".into(),
        kind: if popup {
            CommandKind::Popup
        } else {
            CommandKind::Replace
        },
        prompt: text.trim().to_string(),
        hotkey: None,
        model: None,
    }
}

fn is_adhoc(cmd: &WritingCommand) -> bool {
    cmd.id == ADHOC_ID
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

/// Turn an instruction that was just run into a command worth keeping: label
/// from its first words, id from their slug plus a tail unique among
/// `existing`, the kind it was run as, and no shortcut or model override.
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

/// One row of the picker list. The instruction row exists only while the
/// filter box has text and always sits first, so ↑ from the first match
/// reaches it.
#[derive(Debug, Clone, Copy)]
enum PickerRow<'a> {
    Instruction,
    Command(&'a WritingCommand),
}

fn picker_rows<'a>(commands: &'a [WritingCommand], filter: &str) -> Vec<PickerRow<'a>> {
    let mut rows = Vec::new();
    if !filter.trim().is_empty() {
        rows.push(PickerRow::Instruction);
    }
    rows.extend(
        filter_commands(commands, filter)
            .into_iter()
            .map(PickerRow::Command),
    );
    rows
}

/// Row to highlight after the filter text changes: the first matching command
/// when there is one (so `proof` + ⏎ still runs Proofread), otherwise the
/// instruction row.
fn default_picker_row(rows: &[PickerRow<'_>]) -> usize {
    rows.iter()
        .position(|r| matches!(r, PickerRow::Command(_)))
        .unwrap_or(0)
}

/// The first `max` characters of `text`, with an ellipsis when it was cut.
fn ellipsize(text: &str, max: usize) -> String {
    let short: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        format!("{short}…")
    } else {
        short
    }
}

const DIGIT_KEYS: [egui::Key; 9] = [
    egui::Key::Num1,
    egui::Key::Num2,
    egui::Key::Num3,
    egui::Key::Num4,
    egui::Key::Num5,
    egui::Key::Num6,
    egui::Key::Num7,
    egui::Key::Num8,
    egui::Key::Num9,
];

/// A bare `1`–`9` press this frame as a zero-based row index. The key press and
/// the character it would type are removed from the input so the filter box
/// stays empty; the caller only asks while the filter is empty, so digits typed
/// into a non-empty filter keep filtering.
fn take_digit(input: &mut egui::InputState) -> Option<usize> {
    if !input.modifiers.is_none() {
        return None;
    }
    let idx = DIGIT_KEYS.iter().position(|k| input.key_pressed(*k))?;
    let typed = char::from(b'1' + idx as u8).to_string();
    input.events.retain(|e| {
        !matches!(e, egui::Event::Key { key, .. } if *key == DIGIT_KEYS[idx])
            && !matches!(e, egui::Event::Text(t) if *t == typed)
    });
    Some(idx)
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

/// The two ways a popup result can be written back into the source app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteBack {
    Replace,
    InsertBelow,
}

enum UiPhase {
    Hidden,
    Picker,
    Settings,
    /// A command is running. `partial` accumulates the streamed fragments so
    /// far: rendered as markdown for a Popup, summarised as a character count
    /// for a Replace (whose text is only written back once complete).
    Working {
        label: String,
        kind: CommandKind,
        partial: String,
    },
    Popup {
        title: String,
        body: String,
    },
    Error {
        message: String,
    },
}

struct ServeApp {
    config: AppConfig,
    config_path: PathBuf,
    /// `serve.pid` next to the config file; removed in `on_exit`.
    pidfile: PathBuf,
    selection: Arc<MacosSelection>,
    hotkey: MacosHotkey,
    config_mtime: Option<SystemTime>,
    /// Set from the file-system watcher thread when the config directory
    /// changes; drained once per frame. The watcher is kept alive here.
    config_dirty: Arc<AtomicBool>,
    _config_watcher: Option<notify::RecommendedWatcher>,
    /// Fallback poll so a missed event (or no watcher) still reloads.
    last_config_poll: Instant,
    egui_ctx: egui::Context,
    phase: UiPhase,
    /// Text captured at hotkey time (before our window steals focus).
    captured_text: String,
    captured_app: Option<String>,
    captured_range: Option<(i64, i64)>,
    target_pid: Option<i32>,
    /// Soft-warn acknowledged for the current selection.
    soft_warn_acked: bool,
    /// Replace-size warn acknowledged for the current selection.
    replace_warn_acked: bool,
    /// Command fired by its own shortcut that is waiting on a picker confirmation.
    pending_direct: Option<WritingCommand>,
    settings_status: String,
    /// Bumped whenever the captured selection changes or the window is
    /// dismissed; results from an older generation are discarded.
    generation: u64,
    last_replace: Option<LastReplace>,
    /// The command most recently started, so a popup can Retry it.
    last_command: Option<WritingCommand>,
    md_cache: CommonMarkCache,
    job_rx: Receiver<JobResult>,
    job_tx: Sender<JobResult>,
    runtime: tokio::runtime::Runtime,
    status_line: String,
    /// Live text of the picker's filter box; cleared on every capture.
    picker_filter: String,
    /// Index into the filtered command list that ↑/↓/Enter act on.
    picker_selected: usize,
    /// Give the filter box keyboard focus on the next picker frame.
    focus_filter_next_frame: bool,
    /// Instructions typed into the picker, newest first, for ↑ recall.
    instruction_history: VecDeque<String>,
    /// Which history entry the filter box currently shows, if any; `None`
    /// once the user edits the text or the box is empty again.
    history_cursor: Option<usize>,
    /// Put the filter box's caret at the end on the next frame (after a
    /// recalled instruction replaced its text).
    filter_caret_to_end: bool,
    /// The most recent ad-hoc instruction that finished, kept until it is
    /// saved as a command or another one finishes.
    last_adhoc: Option<WritingCommand>,
    /// One-line feedback in the picker and popup (e.g. "Saved as …").
    picker_notice: String,
}

impl ServeApp {
    fn new(
        _cc: &eframe::CreationContext<'_>,
        config: AppConfig,
        config_path: PathBuf,
        selection: Arc<MacosSelection>,
    ) -> Result<Self> {
        let hotkey = MacosHotkey::new();
        // Wake egui when the hotkey fires so a hidden window still updates.
        let egui_ctx = _cc.egui_ctx.clone();
        let wake_ctx = egui_ctx.clone();
        hotkey.set_wake(move || {
            wake_ctx.request_repaint();
        });
        // Must register on the main thread (eframe creation runs there).
        Self::register_hotkeys(&hotkey, &config)?;
        let config_mtime = std::fs::metadata(&config_path)
            .and_then(|m| m.modified())
            .ok();
        let config_dirty = Arc::new(AtomicBool::new(false));
        let config_watcher = Self::watch_config(&config_path, &config_dirty, &egui_ctx);

        let (job_tx, job_rx) = mpsc::channel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("tokio runtime")?;

        Ok(Self {
            status_line: Self::status_for(&config),
            config,
            pidfile: serve_pidfile(&config_path),
            config_path,
            selection,
            hotkey,
            config_mtime,
            config_dirty,
            _config_watcher: config_watcher,
            last_config_poll: Instant::now(),
            egui_ctx,
            phase: UiPhase::Hidden,
            captured_text: String::new(),
            captured_app: None,
            captured_range: None,
            target_pid: None,
            soft_warn_acked: false,
            replace_warn_acked: false,
            pending_direct: None,
            settings_status: String::new(),
            generation: 0,
            last_replace: None,
            last_command: None,
            md_cache: CommonMarkCache::default(),
            job_rx,
            job_tx,
            runtime,
            picker_filter: String::new(),
            picker_selected: 0,
            focus_filter_next_frame: false,
            instruction_history: VecDeque::new(),
            history_cursor: None,
            filter_caret_to_end: false,
            last_adhoc: None,
            picker_notice: String::new(),
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

    fn show_window(&self, ctx: &egui::Context, visible: bool) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(visible));
        if visible {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
        }
    }

    /// Show the window for a fresh hotkey press: moved next to the mouse cursor
    /// first (on whichever display it is on, inside that display's visible
    /// frame), then made visible. Falls back to the window's last position
    /// when the cursor cannot be located.
    fn show_window_near_cursor(&self, ctx: &egui::Context) {
        if let Some(pos) = self.position_near_cursor(ctx) {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
        self.show_window(ctx, true);
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
            .unwrap_or((f64::from(PICKER_SIZE.0), f64::from(PICKER_SIZE.1)));
        let (x, y) = place_near(cursor, size, visible);
        Some(egui::pos2(x as f32, y as f32))
    }

    fn status_for(config: &AppConfig) -> String {
        let cmd_hk = config
            .commands
            .iter()
            .filter(|c| {
                c.hotkey
                    .as_ref()
                    .map(|h| !h.trim().is_empty())
                    .unwrap_or(false)
            })
            .count();
        let shortcut_hint = config
            .commands
            .iter()
            .find_map(|c| {
                c.hotkey
                    .as_ref()
                    .map(|h| h.trim())
                    .filter(|h| !h.is_empty())
                    .map(|h| format!("{}={}", c.label, h))
            })
            .unwrap_or_else(|| "no cmd shortcuts".into());
        let undo = config
            .undo_hotkey
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(|h| format!(" · Undo: {h}"))
            .unwrap_or_default();
        let excluded = match config.excluded_apps.len() {
            0 => String::new(),
            n => format!(" · {n} excluded apps"),
        };
        format!(
            "Picker: {} · {} cmds · {}{undo}{excluded} · Access: {}",
            config.hotkey,
            config.commands.len(),
            if cmd_hk <= 1 {
                shortcut_hint
            } else {
                format!("{cmd_hk} shortcuts incl. {shortcut_hint}")
            },
            if accessibility_trusted() {
                "ok"
            } else {
                "MISSING"
            }
        )
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
            "selara: registering picker `{}` + {} command shortcut(s)",
            config.hotkey,
            cmd_keys.len()
        );
        for (id, spec) in &cmd_keys {
            tracing::info!("selara:   command `{id}` → `{spec}`");
        }
        let undo = config
            .undo_hotkey
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty());
        if let Some(undo) = undo {
            tracing::info!("selara:   undo → `{undo}`");
        }
        hotkey
            .reregister_all(&config.hotkey, &cmd_keys, undo)
            .with_context(|| format!("register hotkeys (picker `{}`)", config.hotkey))?;
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
        match AppConfig::load_or_init(&self.config_path) {
            Ok(cfg) => {
                if let Err(e) = Self::register_hotkeys(&self.hotkey, &cfg) {
                    self.phase = UiPhase::Error {
                        message: format!("Hotkey reload failed: {e}"),
                    };
                    // still keep new config for limits/commands UI in overlay settings
                }
                self.config = cfg;
                self.config_mtime = Some(mtime);
                self.status_line = Self::status_for(&self.config);
            }
            Err(e) => {
                self.phase = UiPhase::Error {
                    message: format!("Config reload failed: {e}"),
                };
            }
        }
    }

    fn capture_selection(&mut self) -> Result<bool, String> {
        // A new selection supersedes any job still running for the old one.
        self.generation += 1;
        self.target_pid = frontmost_pid();
        self.soft_warn_acked = false;
        self.replace_warn_acked = false;
        self.pending_direct = None;
        self.picker_filter.clear();
        self.picker_selected = 0;
        self.history_cursor = None;
        self.picker_notice.clear();
        self.focus_filter_next_frame = true;
        match self.runtime.block_on(self.selection.read_selection()) {
            Ok(Some(snap)) => {
                self.captured_text = snap.text;
                self.captured_app = snap.app_name;
                self.captured_range = snap.range;
                Ok(true)
            }
            Ok(None) => Err(
                "No text selection found.\nSelect text in another app, then press the hotkey again."
                    .into(),
            ),
            Err(e) => Err(format!("{e}")),
        }
    }

    fn selection_chars(&self) -> u64 {
        self.captured_text.chars().count() as u64
    }

    fn over_hard_max(&self) -> bool {
        let max = self.config.limits.hard_max_chars;
        max > 0 && self.selection_chars() > max
    }

    fn needs_soft_warn(&self) -> bool {
        let soft = self.config.limits.soft_warn_chars;
        soft > 0 && self.selection_chars() > soft && !self.soft_warn_acked
    }

    fn needs_replace_warn(&self) -> bool {
        let warn = self.config.limits.replace_warn_chars;
        warn > 0 && self.selection_chars() > warn && !self.replace_warn_acked
    }

    /// True when the frontmost app is on `excluded_apps`. Checked before any
    /// window, selection, or clipboard access so excluded apps (password
    /// managers, terminals) never see Selara react to the hotkey.
    fn frontmost_is_excluded(&self, trigger: &str) -> bool {
        if self.config.excluded_apps.is_empty() {
            return false;
        }
        let name = frontmost_app_name();
        let bundle = frontmost_bundle_id();
        let excluded = app_is_excluded(
            &self.config.excluded_apps,
            name.as_deref(),
            bundle.as_deref(),
        );
        if excluded {
            tracing::info!(
                "selara: {trigger} ignored, frontmost app is excluded (name={:?}, bundle={:?})",
                name,
                bundle
            );
        }
        excluded
    }

    /// The undo shortcut, guarded like the other two.
    ///
    /// Undo is not a read of the current selection, but it is still one of
    /// Selara's hotkeys, and an excluded app must see nothing from Selara at
    /// all: unguarded it pops our error window over the excluded app when there
    /// is nothing to undo, and otherwise reactivates the earlier target app and
    /// rewrites text there.
    fn on_undo_hotkey(&mut self, ctx: &egui::Context) {
        if self.frontmost_is_excluded("undo hotkey") {
            return;
        }
        self.undo_last_replace(ctx);
    }

    fn on_hotkey(&mut self, ctx: &egui::Context) {
        if self.frontmost_is_excluded("picker hotkey") {
            return;
        }
        if !accessibility_trusted() {
            prompt_accessibility();
            self.phase = UiPhase::Error {
                message: "Accessibility permission missing.\n\n\
System Settings → Privacy & Security → Accessibility\n\
Enable Selara (or Terminal / the binary you launched),\n\
then restart `selara serve`."
                    .into(),
            };
            self.show_window_near_cursor(ctx);
            return;
        }

        match self.capture_selection() {
            Ok(true) => {
                self.phase = UiPhase::Picker;
                self.show_window_near_cursor(ctx);
            }
            Ok(false) => {}
            Err(message) => {
                self.phase = UiPhase::Error { message };
                self.show_window_near_cursor(ctx);
            }
        }
    }

    fn on_command_hotkey(&mut self, ctx: &egui::Context, command_id: &str) {
        if self.frontmost_is_excluded(&format!("command hotkey `{command_id}`")) {
            return;
        }
        if !accessibility_trusted() {
            self.on_hotkey(ctx);
            return;
        }
        let cmd = self
            .config
            .commands
            .iter()
            .find(|c| c.id == command_id)
            .cloned();
        let Some(cmd) = cmd else {
            self.phase = UiPhase::Error {
                message: format!("Unknown command id `{command_id}` for hotkey."),
            };
            self.show_window_near_cursor(ctx);
            return;
        };
        match self.capture_selection() {
            Ok(true) => {
                self.show_window_near_cursor(ctx);
                if self.needs_confirmation(&cmd) {
                    // Same rails as the picker: show it with the banner and run
                    // the command once the user confirms.
                    tracing::info!(
                        "selara: command hotkey `{}` → waiting for confirmation",
                        cmd.id
                    );
                    self.pending_direct = Some(cmd);
                    self.phase = UiPhase::Picker;
                } else {
                    tracing::info!("selara: command hotkey `{}` → running", cmd.id);
                    self.start_command(cmd);
                }
            }
            Ok(false) => {
                self.phase = UiPhase::Error {
                    message: format!(
                        "No text selection for `{}`.

Select text in another app, then press its shortcut again.",
                        cmd.label
                    ),
                };
                self.show_window_near_cursor(ctx);
            }
            Err(message) => {
                self.phase = UiPhase::Error { message };
                self.show_window_near_cursor(ctx);
            }
        }
    }

    /// True when the picker would show a soft-warn or replace-caution banner
    /// for this command on the current selection. The hard max is checked by
    /// `start_command` and is never skippable.
    fn needs_confirmation(&self, cmd: &WritingCommand) -> bool {
        self.needs_soft_warn()
            || (matches!(cmd.kind, CommandKind::Replace) && self.needs_replace_warn())
    }

    /// Run a shortcut-triggered command once nothing blocks it any more. Checked
    /// every frame the picker is shown, so it fires after an acknowledgement
    /// click and also after a limit is raised or disabled in Settings or the
    /// config file. A hard-max block keeps the picker (and its red banner) up.
    fn run_pending_if_ready(&mut self) {
        let ready = self
            .pending_direct
            .as_ref()
            .is_some_and(|cmd| !self.over_hard_max() && !self.needs_confirmation(cmd));
        if ready {
            if let Some(cmd) = self.pending_direct.take() {
                self.start_command(cmd);
            }
        }
    }

    fn hide(&mut self, ctx: &egui::Context) {
        // Dismissing the window (Escape, Close) cancels whatever is in flight:
        // the request keeps running but its result is dropped on arrival.
        self.generation += 1;
        self.pending_direct = None;
        self.phase = UiPhase::Hidden;
        self.show_window(ctx, false);
    }

    fn save_settings(&mut self) {
        match self.config.save(&self.config_path) {
            Ok(()) => {
                if let Err(e) = Self::register_hotkeys(&self.hotkey, &self.config) {
                    self.settings_status = format!("Saved, but hotkey reload failed: {e}");
                } else {
                    self.settings_status = format!("Saved · {}", self.config_path.display());
                }
                self.config_mtime = std::fs::metadata(&self.config_path)
                    .and_then(|m| m.modified())
                    .ok();
                self.status_line = Self::status_for(&self.config);
            }
            Err(e) => {
                self.settings_status = format!("Save failed: {e}");
            }
        }
    }

    fn hard_max_error(&self) -> UiPhase {
        let max = self.config.limits.hard_max_chars;
        UiPhase::Error {
            message: format!(
                "Selection is {} characters — over your hard limit of {max}.\n\n\
Shrink the selection, or raise / disable the limit in Settings (0 = unlimited).",
                self.selection_chars()
            ),
        }
    }

    fn start_command(&mut self, cmd: WritingCommand) {
        // Picking a command by hand supersedes any shortcut-triggered one.
        self.pending_direct = None;
        self.last_command = Some(cmd.clone());
        if self.over_hard_max() {
            self.phase = self.hard_max_error();
            return;
        }
        if self.needs_soft_warn() {
            self.phase = UiPhase::Error {
                message: format!(
                    "Large selection ({} chars) — confirm via the picker, or raise soft warn in Settings.",
                    self.selection_chars()
                ),
            };
            return;
        }
        if matches!(cmd.kind, CommandKind::Replace) && self.needs_replace_warn() {
            self.phase = UiPhase::Error {
                message: format!(
                    "Large replace ({} chars) — confirm via the picker, or raise replace warn in Settings.",
                    self.selection_chars()
                ),
            };
            return;
        }

        let input = self.captured_text.clone();
        let cfg = self.config.clone();
        let tx = self.job_tx.clone();
        let label = cmd.label.clone();
        let generation = self.generation;
        let wake = self.egui_ctx.clone();
        let app_name = self.captured_app.clone();
        self.phase = UiPhase::Working {
            label: label.clone(),
            kind: cmd.kind,
            partial: String::new(),
        };

        self.runtime.spawn(async move {
            let result = async {
                let provider = cfg.build_provider_for(&cmd)?;
                let vars = PromptVars {
                    language: Some(&cfg.language),
                    app: app_name.as_deref(),
                };
                // Every fragment goes straight to the UI thread; the channel
                // is unbounded and the UI drains it once per frame, so no
                // coalescing is needed. Fragments carry the generation so a
                // stale stream (Escape, new hotkey) is dropped like a result.
                let delta_tx = tx.clone();
                let delta_wake = wake.clone();
                let mut on_delta = move |text: &str| {
                    let _ = delta_tx.send(JobResult::Delta {
                        generation,
                        text: text.to_string(),
                    });
                    delta_wake.request_repaint();
                };
                let out =
                    run_command_stream(provider.as_ref(), &cmd, &input, None, vars, &mut on_delta)
                        .await?;
                Ok::<_, anyhow::Error>((cmd.kind, cmd.label, out))
            }
            .await;

            let msg = match result {
                Ok((kind, label, text)) => JobResult::Success {
                    generation,
                    kind,
                    label,
                    text,
                },
                Err(e) => JobResult::Error {
                    generation,
                    message: e.to_string(),
                },
            };
            let _ = tx.send(msg);
            // The UI may be idling at a slow tick; make it pick the result up now.
            wake.request_repaint();
        });
    }

    fn apply_job(&mut self, ctx: &egui::Context, job: JobResult) {
        match job {
            JobResult::Delta { text, .. } => {
                // `should_apply_job` already checked the phase is Working for
                // this generation; anything else means the fragment is stale.
                if let UiPhase::Working { partial, .. } = &mut self.phase {
                    partial.push_str(&text);
                }
            }
            JobResult::Error { message, .. } => {
                self.phase = UiPhase::Error { message };
            }
            JobResult::Success {
                kind, label, text, ..
            } => {
                // A finished instruction is offered for saving: from the
                // popup right away, or from the picker's banner next time.
                if self.last_command.as_ref().is_some_and(is_adhoc) {
                    self.last_adhoc = self.last_command.clone();
                }
                match kind {
                    CommandKind::Popup => {
                        self.phase = UiPhase::Popup {
                            title: label,
                            body: text,
                        };
                    }
                    CommandKind::Replace => self.replace_selection_with(ctx, text),
                }
            }
        }
    }

    /// Run a command picked in the picker. An ad-hoc instruction is also
    /// remembered for ↑ recall.
    fn run_picked(&mut self, cmd: WritingCommand) {
        if is_adhoc(&cmd) {
            push_history(&mut self.instruction_history, &cmd.prompt);
            tracing::info!(
                "selara: running instruction ({} chars) as {:?}",
                cmd.prompt.chars().count(),
                cmd.kind
            );
        }
        self.start_command(cmd);
    }

    /// Append the last finished instruction to the config as a real command
    /// and save the file. On failure the command is dropped again and the
    /// instruction stays offered.
    fn save_last_adhoc(&mut self) {
        let Some(adhoc) = self.last_adhoc.take() else {
            return;
        };
        let existing: Vec<String> = self.config.commands.iter().map(|c| c.id.clone()).collect();
        let cmd = command_from_instruction(&adhoc.prompt, adhoc.kind, &existing);
        let (id, label) = (cmd.id.clone(), cmd.label.clone());
        self.config.commands.push(cmd);
        match self.config.save(&self.config_path) {
            Ok(()) => {
                self.config_mtime = std::fs::metadata(&self.config_path)
                    .and_then(|m| m.modified())
                    .ok();
                self.status_line = Self::status_for(&self.config);
                self.picker_notice = format!("Saved as “{label}” · edit it in Settings → Commands");
                tracing::info!("selara: saved instruction as command `{id}`");
            }
            Err(e) => {
                self.config.commands.pop();
                self.last_adhoc = Some(adhoc);
                self.picker_notice = format!("Save failed: {e}");
            }
        }
    }

    /// Hide, hand focus back to the source app, and write `text` over the
    /// captured selection. Shared by Replace commands and the popup's
    /// "Replace selection" button.
    fn replace_selection_with(&mut self, ctx: &egui::Context, text: String) {
        let pid = self.target_pid;
        let original = self.captured_text.clone();
        let range = self.captured_range;
        self.refocus_target(ctx);
        match self.selection.replace_in_app(pid, &text, &original, range) {
            Ok(()) => {
                self.last_replace = Some(LastReplace {
                    pid,
                    original,
                    replacement: text,
                    range,
                });
            }
            Err(e) => {
                self.phase = UiPhase::Error {
                    message: format!("Replace failed: {e}"),
                };
                self.show_window(ctx, true);
            }
        }
    }

    /// Hide, hand focus back to the source app, and insert `body` after the
    /// captured selection (separated by a blank line). Recorded as a
    /// zero-length "replace" so Undo removes exactly what was inserted.
    fn insert_below_selection(&mut self, ctx: &egui::Context, body: &str) {
        let pid = self.target_pid;
        let captured_range = self.captured_range;
        let text = insert_text(body);
        self.refocus_target(ctx);
        match self
            .selection
            .insert_after_selection(pid, &text, captured_range)
        {
            Ok(()) => {
                self.last_replace = Some(LastReplace {
                    pid,
                    original: String::new(),
                    replacement: text,
                    range: captured_range.map(insert_range),
                });
            }
            Err(e) => {
                self.phase = UiPhase::Error {
                    message: format!("Insert failed: {e}"),
                };
                self.show_window(ctx, true);
            }
        }
    }

    /// Popup button handler: same rails as `start_command` for a Replace, then
    /// write the result back. The caution disables the buttons until it is
    /// acknowledged, so only the hard max needs re-checking here.
    fn write_back_from_popup(&mut self, ctx: &egui::Context, body: String, how: WriteBack) {
        if self.over_hard_max() {
            self.phase = self.hard_max_error();
            return;
        }
        if self.needs_replace_warn() {
            return;
        }
        match how {
            WriteBack::Replace => self.replace_selection_with(ctx, body),
            WriteBack::InsertBelow => self.insert_below_selection(ctx, &body),
        }
    }

    /// Hide first so macOS can restore focus to the source app, then activate
    /// it. Pasting while we are still frontmost fails.
    fn refocus_target(&mut self, ctx: &egui::Context) {
        let pid = self.target_pid;
        self.hide(ctx);
        std::thread::sleep(std::time::Duration::from_millis(80));
        if let Some(pid) = pid {
            let _ = activate_pid(pid);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(180));
        }
    }
}

impl ServeApp {
    /// Put the last replaced selection back. Works from the undo hotkey and the
    /// picker button; the source app is re-activated first, like a Replace.
    fn undo_last_replace(&mut self, ctx: &egui::Context) {
        // Check permission before taking the record: an early return here used
        // to drop it, so re-granting Accessibility left nothing to undo.
        if !accessibility_trusted() {
            self.on_hotkey(ctx);
            return;
        }
        let Some(last) = self.last_replace.take() else {
            self.phase = UiPhase::Error {
                message: "Nothing to undo: no Replace has run since Selara started.".into(),
            };
            self.show_window(ctx, true);
            return;
        };
        self.hide(ctx);
        std::thread::sleep(std::time::Duration::from_millis(80));
        if let Some(pid) = last.pid {
            let _ = activate_pid(pid);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(180));
        }
        tracing::info!("selara: undo last replace ({} chars)", last.original.len());
        if let Err(e) =
            self.selection
                .undo_replace(last.pid, &last.original, &last.replacement, last.range)
        {
            // Keep it so the user can retry after fixing focus.
            self.last_replace = Some(last);
            self.phase = UiPhase::Error {
                message: format!("Undo failed: {e}"),
            };
            self.show_window(ctx, true);
        }
    }
}

impl eframe::App for ServeApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        remove_pidfile(&self.pidfile);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.hotkey.poll();
        self.poll_config();

        if let Some(action) = self.hotkey.take_pending() {
            match action {
                HotkeyAction::Picker => self.on_hotkey(ctx),
                HotkeyAction::Command(id) => self.on_command_hotkey(ctx, &id),
                HotkeyAction::Undo => self.on_undo_hotkey(ctx),
            }
        }

        while let Ok(job) = self.job_rx.try_recv() {
            let waiting = matches!(self.phase, UiPhase::Working { .. });
            if should_apply_job(job.generation(), self.generation, waiting) {
                self.apply_job(ctx, job);
            } else {
                tracing::info!(
                    job = job.generation(),
                    current = self.generation,
                    waiting,
                    "selara: dropping stale job result"
                );
            }
        }

        if matches!(self.phase, UiPhase::Working { .. }) {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        if matches!(self.phase, UiPhase::Hidden) {
            // Hotkeys, config changes, and job results all wake the loop
            // explicitly; this slow tick only backs up `hotkey.poll()` and the
            // 5 s config poll, so an idle `serve` stays near zero CPU.
            ctx.request_repaint_after(Duration::from_secs(1));
            egui::CentralPanel::default().show(ctx, |_ui| {});
            return;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if matches!(self.phase, UiPhase::Settings) {
                self.phase = UiPhase::Picker;
                self.focus_filter_next_frame = true;
            } else {
                self.hide(ctx);
            }
            return;
        }

        // Keyboard-first picker: ↑/↓ move the highlight, Enter runs it, and a
        // bare 1–9 runs that row while the filter box is empty. Text that is
        // typed is also an instruction: ⌘⏎ always runs it (⇧⏎ as a popup), and
        // so does ⏎ on the "Run instruction" row or when nothing matches. ↑ in
        // an empty box recalls the previous instruction. The keys are consumed
        // here, before the panel renders, so the filter box never sees them
        // (Enter would otherwise drop its focus).
        let mut key_run: Option<WritingCommand> = None;
        let mut selection_moved = false;
        if matches!(self.phase, UiPhase::Picker) {
            let filter_empty = self.picker_filter.trim().is_empty();
            let (up, down, cmd_enter, shift_enter, enter, digit) = ctx.input_mut(|i| {
                let up = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
                let down = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
                // Most specific first: `consume_key` ignores an extra Shift,
                // so the bare-Enter check would otherwise swallow ⇧⏎ too.
                let cmd_enter = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter);
                let shift_enter = i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter);
                let enter = i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                let digit = if filter_empty { take_digit(i) } else { None };
                (up, down, cmd_enter, shift_enter, enter, digit)
            });
            // While the box is empty, or still shows a recalled instruction,
            // ↑/↓ walk the history instead of the rows.
            let walking_history = (up || down)
                && (filter_empty || self.history_cursor.is_some())
                && !self.instruction_history.is_empty();
            if walking_history {
                let delta = if up { 1 } else { -1 };
                self.history_cursor =
                    history_step(self.history_cursor, self.instruction_history.len(), delta);
                self.picker_filter = self
                    .history_cursor
                    .and_then(|i| self.instruction_history.get(i).cloned())
                    .unwrap_or_default();
                self.filter_caret_to_end = true;
                // A recalled instruction is meant to run as one.
                self.picker_selected = 0;
            }
            let rows = picker_rows(&self.config.commands, &self.picker_filter);
            let len = rows.len();
            let mut selected = clamp_selection(self.picker_selected, len);
            if !walking_history {
                if up {
                    selected = next_selection(selected, len, -1);
                    selection_moved = true;
                }
                if down {
                    selected = next_selection(selected, len, 1);
                    selection_moved = true;
                }
            }
            let has_instruction = !self.picker_filter.trim().is_empty();
            let target: Option<(PickerRow<'_>, bool)> = if cmd_enter && has_instruction {
                Some((PickerRow::Instruction, false))
            } else if shift_enter && has_instruction {
                Some((PickerRow::Instruction, true))
            } else if enter {
                rows.get(selected).map(|r| (*r, false))
            } else {
                digit.and_then(|d| rows.get(d)).map(|r| (*r, false))
            };
            if let Some((row, popup)) = target {
                let cmd = match row {
                    PickerRow::Instruction => adhoc_command(&self.picker_filter, popup),
                    PickerRow::Command(cmd) => cmd.clone(),
                };
                let enabled = picker_row_enabled(
                    matches!(cmd.kind, CommandKind::Replace),
                    self.over_hard_max(),
                    self.needs_soft_warn(),
                    self.needs_replace_warn(),
                );
                if enabled {
                    key_run = Some(cmd);
                }
            }
            self.picker_selected = selected;
        }
        let focus_filter = matches!(self.phase, UiPhase::Picker)
            && std::mem::take(&mut self.focus_filter_next_frame);

        // Collect click target without holding a borrow across mutation.
        let mut clicked: Option<WritingCommand> = key_run;
        let mut dismiss = false;
        let mut open_settings = false;
        let mut back_to_picker = false;
        let mut save_settings = false;
        let mut reset_limits = false;
        let mut ack_soft = false;
        let mut ack_replace = false;
        let mut undo = false;
        let mut write_back: Option<(WriteBack, String)> = None;
        let mut retry = false;
        let mut save_adhoc = false;

        let soft_blocked = matches!(self.phase, UiPhase::Picker) && self.needs_soft_warn();
        let hard_blocked = matches!(self.phase, UiPhase::Picker) && self.over_hard_max();
        let replace_caution = matches!(self.phase, UiPhase::Picker) && self.needs_replace_warn();
        let popup_actions = popup_actions(self.over_hard_max(), self.needs_replace_warn());
        let can_retry = self.last_command.is_some();
        // The popup shows "Save as command…" only for the instruction it
        // displays, not for a configured command run after an instruction.
        let popup_from_adhoc =
            self.last_adhoc.is_some() && self.last_command.as_ref().is_some_and(is_adhoc);
        let filter_caret_to_end = std::mem::take(&mut self.filter_caret_to_end);

        egui::CentralPanel::default().show(ctx, |ui| {
            // The window has no OS title bar; the header row is the drag
            // handle. Registered before the buttons so they stay on top.
            let header_rect = {
                let mut r = ui.max_rect();
                r.max.y = r.min.y + 28.0;
                r
            };
            let drag = ui.interact(
                header_rect,
                ui.id().with("header_drag"),
                egui::Sense::click_and_drag(),
            );
            if drag.drag_started_by(egui::PointerButton::Primary) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.horizontal(|ui| {
                ui.heading("Selara").on_hover_text("Drag to move");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        dismiss = true;
                    }
                    if !matches!(self.phase, UiPhase::Settings | UiPhase::Working { .. })
                        && ui.button("Settings").clicked()
                    {
                        open_settings = true;
                    }
                    if matches!(self.phase, UiPhase::Settings) && ui.button("Back").clicked() {
                        back_to_picker = true;
                    }
                    if self.last_replace.is_some()
                        && matches!(
                            self.phase,
                            UiPhase::Picker | UiPhase::Error { .. } | UiPhase::Popup { .. }
                        )
                        && ui
                            .button("Undo last replace")
                            .on_hover_text("Put back the text the previous Replace overwrote")
                            .clicked()
                    {
                        undo = true;
                    }
                });
            });
            ui.label(&self.status_line);
            if let Some(app) = &self.captured_app {
                if !matches!(self.phase, UiPhase::Settings) {
                    ui.label(format!("From: {app}"));
                }
            }
            ui.separator();

            match &self.phase {
                UiPhase::Picker => {
                    let chars = self.selection_chars();
                    ui.label(format!("Selection ({chars} chars)"));
                    if let Some(pending) = &self.pending_direct {
                        ui.small(format!(
                            "{} was triggered by its shortcut and will run once you confirm below.",
                            pending.label
                        ));
                    }
                    ui.small(ellipsize(&self.captured_text, 220));
                    if let Some(adhoc) = &self.last_adhoc {
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.small(format!(
                                "Last instruction: {}",
                                ellipsize(&adhoc.prompt, 60)
                            ));
                            if ui
                                .small_button("Save as command…")
                                .on_hover_text(
                                    "Add it to your commands, named after its first words",
                                )
                                .clicked()
                            {
                                save_adhoc = true;
                            }
                        });
                    }
                    if !self.picker_notice.is_empty() {
                        ui.small(&self.picker_notice);
                    }

                    if hard_blocked {
                        ui.add_space(6.0);
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 80, 80),
                            format!(
                                "Over hard limit ({}). Shrink selection or raise the limit in Settings (0 = unlimited).",
                                self.config.limits.hard_max_chars
                            ),
                        );
                    } else if soft_blocked {
                        ui.add_space(6.0);
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 150, 40),
                            format!(
                                "Large selection (soft warn at {} chars). May be slower / cost more on your API key.",
                                self.config.limits.soft_warn_chars
                            ),
                        );
                        if ui.button("Continue anyway").clicked() {
                            ack_soft = true;
                        }
                    } else if replace_caution {
                        ui.add_space(6.0);
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 150, 40),
                            format!(
                                "Replace caution ({}+ chars): paste-back can be flaky in some apps. Popup commands are safer.",
                                self.config.limits.replace_warn_chars
                            ),
                        );
                        if ui.button("I understand — allow Replace").clicked() {
                            ack_replace = true;
                        }
                    }

                    ui.add_space(8.0);
                    let filter = egui::TextEdit::singleline(&mut self.picker_filter)
                        .hint_text("Filter, or type an instruction · ↑ recalls the last one")
                        .desired_width(f32::INFINITY)
                        .show(ui);
                    if focus_filter {
                        filter.response.request_focus();
                    }
                    if filter_caret_to_end {
                        let mut state = filter.state;
                        let end = egui::text::CCursor::new(self.picker_filter.chars().count());
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(end)));
                        state.store(ui.ctx(), filter.response.id);
                    }
                    let filter_changed = filter.response.changed();
                    ui.add_space(4.0);

                    let commands = self.config.commands.clone();
                    let filter_text = self.picker_filter.clone();
                    let rows = picker_rows(&commands, &filter_text);
                    if filter_changed {
                        // Typing over a recalled instruction ends the walk.
                        self.history_cursor = None;
                        self.picker_selected = default_picker_row(&rows);
                    }
                    let selected_idx = clamp_selection(self.picker_selected, rows.len());
                    self.picker_selected = selected_idx;
                    let quick_pick = filter_text.trim().is_empty();
                    let no_command_matches =
                        !rows.iter().any(|r| matches!(r, PickerRow::Command(_)));
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (idx, row) in rows.iter().enumerate() {
                                let selected = idx == selected_idx;
                                let cmd = match row {
                                    PickerRow::Command(cmd) => *cmd,
                                    PickerRow::Instruction => {
                                        let run_enabled = picker_row_enabled(
                                            true,
                                            hard_blocked,
                                            soft_blocked,
                                            replace_caution,
                                        );
                                        let popup_enabled = picker_row_enabled(
                                            false,
                                            hard_blocked,
                                            soft_blocked,
                                            replace_caution,
                                        );
                                        let resp = ui.add_enabled(
                                            run_enabled,
                                            egui::Button::selectable(
                                                selected,
                                                (
                                                    egui::RichText::new("⏎").weak().monospace(),
                                                    egui::RichText::new("Run instruction")
                                                        .strong(),
                                                ),
                                            )
                                            .right_text(
                                                egui::RichText::new("replace").weak().small(),
                                            )
                                            .min_size(egui::vec2(ui.available_width(), 28.0)),
                                        );
                                        if selected && selection_moved {
                                            resp.scroll_to_me(None);
                                        }
                                        if resp.clicked() {
                                            clicked = Some(adhoc_command(&filter_text, false));
                                        }
                                        ui.horizontal_wrapped(|ui| {
                                            ui.small(
                                                "⏎ run as Replace · ⇧⏎ run as Popup · ⌘⏎ always runs the instruction",
                                            );
                                            if ui
                                                .add_enabled(
                                                    popup_enabled,
                                                    egui::Button::new(
                                                        egui::RichText::new("Run as Popup")
                                                            .small(),
                                                    ),
                                                )
                                                .clicked()
                                            {
                                                clicked = Some(adhoc_command(&filter_text, true));
                                            }
                                        });
                                        if no_command_matches {
                                            ui.small("No command matches; ⏎ runs the text as an instruction.");
                                        }
                                        ui.add_space(4.0);
                                        continue;
                                    }
                                };
                                let kind_tag = match cmd.kind {
                                    CommandKind::Replace => "replace",
                                    CommandKind::Popup => "popup",
                                };
                                let enabled = picker_row_enabled(
                                    matches!(cmd.kind, CommandKind::Replace),
                                    hard_blocked,
                                    soft_blocked,
                                    replace_caution,
                                );
                                // 1–9 only work while the box is empty, so the
                                // badges are shown only then.
                                let badge = if quick_pick && idx < 9 {
                                    format!("{}", idx + 1)
                                } else {
                                    " ".to_string()
                                };
                                let resp = ui.add_enabled(
                                    enabled,
                                    egui::Button::selectable(
                                        selected,
                                        (
                                            egui::RichText::new(badge).weak().monospace(),
                                            egui::RichText::new(cmd.label.as_str()),
                                        ),
                                    )
                                    .right_text(egui::RichText::new(kind_tag).weak().small())
                                    .min_size(egui::vec2(ui.available_width(), 28.0)),
                                );
                                if selected && selection_moved {
                                    resp.scroll_to_me(None);
                                }
                                if resp.clicked() {
                                    clicked = Some(cmd.clone());
                                }
                            }
                        });
                }
                UiPhase::Settings => {
                    ui.label("Limits");
                    ui.small("Defaults protect against huge accidental pastes. Set any value to 0 to disable that rail. Saved to your config — no TOML editing required.");
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label("Soft warn (chars)");
                        ui.add(
                            egui::DragValue::new(&mut self.config.limits.soft_warn_chars)
                                .speed(100)
                                .range(0..=2_000_000),
                        );
                    });
                    ui.small("Show a confirm step above this size.");

                    ui.horizontal(|ui| {
                        ui.label("Hard max (chars)");
                        ui.add(
                            egui::DragValue::new(&mut self.config.limits.hard_max_chars)
                                .speed(500)
                                .range(0..=5_000_000),
                        );
                    });
                    ui.small("Refuse to send above this size. 0 = unlimited.");

                    ui.horizontal(|ui| {
                        ui.label("Replace caution (chars)");
                        ui.add(
                            egui::DragValue::new(&mut self.config.limits.replace_warn_chars)
                                .speed(100)
                                .range(0..=2_000_000),
                        );
                    });
                    ui.small("Extra confirm before Replace on large selections.");

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            save_settings = true;
                        }
                        if ui.button("Reset defaults").clicked() {
                            reset_limits = true;
                        }
                    });
                    if !self.settings_status.is_empty() {
                        ui.small(&self.settings_status);
                    }
                    ui.add_space(8.0);
                    ui.small(format!("Config file: {}", self.config_path.display()));
                }
                UiPhase::Working {
                    label,
                    kind,
                    partial,
                } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Running {label}…"));
                    });
                    if !partial.is_empty() {
                        ui.add_space(6.0);
                        match kind {
                            // Same viewer as the finished popup, so the text
                            // does not re-flow when Success replaces it.
                            CommandKind::Popup => {
                                egui::ScrollArea::vertical()
                                    .max_height(360.0)
                                    .stick_to_bottom(true)
                                    .show(ui, |ui| {
                                        CommonMarkViewer::new().show(
                                            ui,
                                            &mut self.md_cache,
                                            partial,
                                        );
                                    });
                            }
                            CommandKind::Replace => {
                                ui.small(replace_progress(partial));
                            }
                        }
                    }
                }
                UiPhase::Popup { title, body } => {
                    ui.heading(title);
                    ui.add_space(6.0);
                    // Lay the action row out from the bottom edge up, then give
                    // the result scroller only what is left. With a fixed
                    // max_height a long result pushed Copy / Replace selection /
                    // Insert below / Retry past the bottom of the borderless
                    // window, where they could be neither clicked nor scrolled
                    // to without resizing the window by hand.
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                        if popup_actions.show_caution {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(
                                    egui::Color32::from_rgb(200, 150, 40),
                                    format!(
                                        "Replace caution ({}+ chars): paste-back can be flaky in some apps.",
                                        self.config.limits.replace_warn_chars
                                    ),
                                );
                                if ui.button("Allow replace").clicked() {
                                    ack_replace = true;
                                }
                            });
                            ui.add_space(4.0);
                        }
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .button("Copy")
                                .on_hover_text("Copy the result as markdown")
                                .clicked()
                            {
                                ui.ctx().copy_text(body.clone());
                            }
                            if ui
                                .add_enabled(
                                    popup_actions.replace_enabled,
                                    egui::Button::new("Replace selection"),
                                )
                                .on_hover_text("Write the result over the original selection")
                                .clicked()
                            {
                                write_back = Some((WriteBack::Replace, body.clone()));
                            }
                            if ui
                                .add_enabled(
                                    popup_actions.insert_enabled,
                                    egui::Button::new("Insert below"),
                                )
                                .on_hover_text(if self.captured_range.is_some() {
                                    "Insert the result after the selection"
                                } else {
                                    "Insert the result after the selection (moves the caret with →, then pastes)"
                                })
                                .clicked()
                            {
                                write_back = Some((WriteBack::InsertBelow, body.clone()));
                            }
                            if ui
                                .add_enabled(can_retry, egui::Button::new("Retry"))
                                .on_hover_text("Run the same command again on the same selection")
                                .clicked()
                            {
                                retry = true;
                            }
                            if popup_from_adhoc
                                && ui
                                    .button("Save as command…")
                                    .on_hover_text(
                                        "Add this instruction to your commands, named after its first words",
                                    )
                                    .clicked()
                            {
                                save_adhoc = true;
                            }
                        });
                        if !self.picker_notice.is_empty() {
                            ui.small(&self.picker_notice);
                        }
                        ui.add_space(6.0);
                        // Whatever is left above the footer, top-down again so
                        // the result reads normally.
                        let remaining = ui.available_height().max(60.0);
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            egui::ScrollArea::vertical().max_height(remaining).show(
                                ui,
                                |ui| {
                                    CommonMarkViewer::new().show(ui, &mut self.md_cache, body);
                                },
                            );
                        });
                    });
                }
                UiPhase::Error { message } => {
                    ui.colored_label(egui::Color32::from_rgb(200, 80, 80), "Error");
                    ui.label(message);
                    ui.horizontal(|ui| {
                        if ui.button("Dismiss").clicked() {
                            dismiss = true;
                        }
                        if ui.button("Settings").clicked() {
                            open_settings = true;
                        }
                    });
                }
                UiPhase::Hidden => {}
            }
        });

        if ack_soft {
            self.soft_warn_acked = true;
        }
        if ack_replace {
            self.replace_warn_acked = true;
        }
        if open_settings {
            self.settings_status.clear();
            self.phase = UiPhase::Settings;
        }
        if back_to_picker {
            self.phase = UiPhase::Picker;
            self.focus_filter_next_frame = true;
        }
        if reset_limits {
            self.config.limits = LimitsConfig::default();
            self.settings_status = "Defaults restored (not saved yet)".into();
        }
        if save_settings {
            self.save_settings();
        }
        if save_adhoc {
            self.save_last_adhoc();
        }
        if dismiss {
            self.hide(ctx);
        }
        if undo {
            self.undo_last_replace(ctx);
            return;
        }
        if let Some((how, body)) = write_back {
            self.write_back_from_popup(ctx, body, how);
            return;
        }
        if retry {
            if let Some(cmd) = self.last_command.clone() {
                self.start_command(cmd);
            }
            return;
        }
        if let Some(cmd) = clicked {
            self.run_picked(cmd);
        }
        if matches!(self.phase, UiPhase::Picker) {
            self.run_pending_if_ready();
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

pub fn run(config_path: PathBuf) -> Result<()> {
    let config = AppConfig::load_or_init(&config_path)?;
    write_pidfile(&serve_pidfile(&config_path))?;
    println!("config: {}", config_path.display());
    println!("hotkey: {}", config.hotkey);
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
    if cmd_shortcuts.is_empty() {
        println!("command shortcuts: (none)");
    } else {
        println!("command shortcuts: {}", cmd_shortcuts.join(", "));
    }
    println!(
        "limits: soft_warn={} hard_max={} replace_warn={}",
        config.limits.soft_warn_chars,
        config.limits.hard_max_chars,
        config.limits.replace_warn_chars
    );
    println!(
        "accessibility: {}",
        if accessibility_trusted() {
            "granted"
        } else {
            "MISSING — grant under System Settings → Privacy & Security → Accessibility"
        }
    );
    if !accessibility_trusted() {
        prompt_accessibility();
    }

    let selection = Arc::new(MacosSelection::new()?);
    let config_path_for_app = config_path.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([PICKER_SIZE.0, PICKER_SIZE.1])
            .with_min_inner_size([320.0, 300.0])
            .with_resizable(true)
            // Borderless: the header row inside the panel is the drag handle
            // (`ViewportCommand::StartDrag`), and Esc / Close dismiss it.
            .with_decorations(false)
            .with_always_on_top()
            .with_visible(false)
            .with_title("Selara"),
        ..Default::default()
    };

    eframe::run_native(
        "Selara",
        options,
        Box::new(move |cc| {
            Ok(
                Box::new(ServeApp::new(cc, config, config_path_for_app, selection)?)
                    as Box<dyn eframe::App>,
            )
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{
        adhoc_command, clamp_selection, command_from_instruction, default_picker_row, ellipsize,
        filter_commands, format_thousands, history_step, insert_range, insert_text,
        instruction_label, next_selection, picker_row_enabled, picker_rows, place_near,
        popup_actions, push_history, remove_pidfile, replace_progress, should_apply_job, slugify,
        unique_command_id, write_pidfile, PickerRow, PopupActions, ADHOC_ID, HISTORY_CAP,
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

    use selara_core::commands::{CommandKind, WritingCommand};

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

    fn cmd(label: &str, prompt: &str) -> WritingCommand {
        WritingCommand {
            id: label.to_lowercase(),
            label: label.into(),
            kind: CommandKind::Replace,
            prompt: prompt.into(),
            hotkey: None,
            model: None,
        }
    }

    fn sample() -> Vec<WritingCommand> {
        vec![
            cmd("Proofread", "Fix grammar and spelling."),
            cmd("Summary", "Summarize the text."),
            cmd("Professional", "Rewrite the text in a professional tone."),
        ]
    }

    fn labels<'a>(list: &[&'a WritingCommand]) -> Vec<&'a str> {
        list.iter().map(|c| c.label.as_str()).collect()
    }

    #[test]
    fn empty_or_blank_query_returns_every_command() {
        let all = sample();
        assert_eq!(filter_commands(&all, "").len(), 3);
        assert_eq!(filter_commands(&all, "   ").len(), 3);
    }

    #[test]
    fn filter_matches_labels_case_insensitively() {
        let all = sample();
        assert_eq!(
            labels(&filter_commands(&all, "PRO")),
            vec!["Proofread", "Professional"]
        );
        assert_eq!(labels(&filter_commands(&all, " summ ")), vec!["Summary"]);
    }

    #[test]
    fn filter_falls_back_to_prompts_only_when_no_label_matches() {
        let all = sample();
        // "grammar" is in Proofread's prompt only.
        assert_eq!(labels(&filter_commands(&all, "grammar")), vec!["Proofread"]);
        // "text" is in two prompts but also in no label.
        assert_eq!(
            labels(&filter_commands(&all, "text")),
            vec!["Summary", "Professional"]
        );
        // A label match wins even though "pro" also appears in a prompt.
        assert_eq!(
            labels(&filter_commands(&all, "professional")),
            vec!["Professional"]
        );
        assert!(filter_commands(&all, "zzz").is_empty());
    }

    #[test]
    fn next_selection_wraps_around_in_both_directions() {
        assert_eq!(next_selection(0, 3, 1), 1);
        assert_eq!(next_selection(2, 3, 1), 0);
        assert_eq!(next_selection(0, 3, -1), 2);
        assert_eq!(next_selection(1, 3, -1), 0);
        assert_eq!(next_selection(0, 0, 1), 0);
        assert_eq!(next_selection(5, 0, -1), 0);
    }

    #[test]
    fn clamp_selection_keeps_the_highlight_inside_the_filtered_list() {
        assert_eq!(clamp_selection(7, 3), 2);
        assert_eq!(clamp_selection(1, 3), 1);
        assert_eq!(clamp_selection(4, 0), 0);
    }

    #[test]
    fn picker_rows_follow_the_same_rails_as_the_buttons() {
        // Nothing blocked: everything runs.
        assert!(picker_row_enabled(true, false, false, false));
        assert!(picker_row_enabled(false, false, false, false));
        // Hard max or soft warn block every row.
        assert!(!picker_row_enabled(false, true, false, false));
        assert!(!picker_row_enabled(false, false, true, false));
        // Replace caution blocks Replace rows only.
        assert!(!picker_row_enabled(true, false, false, true));
        assert!(picker_row_enabled(false, false, false, true));
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

    #[test]
    fn popup_actions_are_all_enabled_within_limits() {
        assert_eq!(
            popup_actions(false, false),
            PopupActions {
                replace_enabled: true,
                insert_enabled: true,
                show_caution: false,
            }
        );
    }

    #[test]
    fn popup_actions_lock_write_back_until_caution_is_acknowledged() {
        assert_eq!(
            popup_actions(false, true),
            PopupActions {
                replace_enabled: false,
                insert_enabled: false,
                show_caution: true,
            }
        );
    }

    #[test]
    fn popup_actions_over_hard_max_stay_clickable_so_the_click_can_explain() {
        // The hard max cannot be acknowledged away, so no caution button is
        // offered; the click path shows the hard-max error instead.
        for needs_replace_warn in [false, true] {
            assert_eq!(
                popup_actions(true, needs_replace_warn),
                PopupActions {
                    replace_enabled: true,
                    insert_enabled: true,
                    show_caution: false,
                }
            );
        }
    }

    #[test]
    fn insert_range_is_a_caret_at_the_end_of_the_selection() {
        assert_eq!(insert_range((10, 5)), (15, 0));
        assert_eq!(insert_range((0, 0)), (0, 0));
    }

    #[test]
    fn insert_text_separates_the_result_with_a_blank_line() {
        assert_eq!(insert_text("- a\n- b"), "\n\n- a\n- b");
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
    fn adhoc_command_runs_the_typed_text_as_replace_or_popup() {
        let replace = adhoc_command("  make it shorter ", false);
        assert_eq!(replace.id, ADHOC_ID);
        assert_eq!(replace.label, "Instruction");
        assert_eq!(replace.prompt, "make it shorter");
        assert!(matches!(replace.kind, CommandKind::Replace));
        assert!(replace.hotkey.is_none() && replace.model.is_none());
        let popup = adhoc_command("explain this", true);
        assert!(matches!(popup.kind, CommandKind::Popup));
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
            CommandKind::Popup,
            &existing,
        );
        assert_eq!(cmd.label, "Make it shorter and");
        assert_eq!(cmd.prompt, "Make it shorter and punchier");
        assert!(cmd.id.starts_with("make-it-shorter-and-"), "{}", cmd.id);
        assert!(!existing.contains(&cmd.id));
        assert!(matches!(cmd.kind, CommandKind::Popup));
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
    fn picker_rows_put_the_instruction_first_only_while_there_is_text() {
        let all = sample();
        let empty = picker_rows(&all, "  ");
        assert_eq!(empty.len(), 3);
        assert!(matches!(empty[0], PickerRow::Command(_)));
        assert_eq!(default_picker_row(&empty), 0);

        let matching = picker_rows(&all, "pro");
        assert_eq!(matching.len(), 3);
        assert!(matches!(matching[0], PickerRow::Instruction));
        assert!(matches!(matching[1], PickerRow::Command(c) if c.label == "Proofread"));
        // ⏎ still runs the first matching command; ↑ reaches the instruction.
        assert_eq!(default_picker_row(&matching), 1);

        let none = picker_rows(&all, "make it rhyme");
        assert_eq!(none.len(), 1);
        assert!(matches!(none[0], PickerRow::Instruction));
        assert_eq!(default_picker_row(&none), 0);
    }

    #[test]
    fn ellipsize_cuts_on_characters() {
        assert_eq!(ellipsize("abc", 5), "abc");
        assert_eq!(ellipsize("ééééé", 3), "ééé…");
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
}
