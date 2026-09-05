//! macOS desktop shell: global hotkey → command picker → replace / popup.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use eframe::egui;
use selara_core::commands::{run_command, CommandKind, WritingCommand};
use selara_core::config::{AppConfig, LimitsConfig};
use selara_platform::macos::{
    accessibility_trusted, activate_pid, frontmost_pid, prompt_accessibility, HotkeyAction,
    MacosHotkey, MacosSelection,
};
use selara_platform::SelectionService;

#[derive(Debug)]
enum JobResult {
    Success {
        kind: CommandKind,
        label: String,
        text: String,
    },
    Error(String),
}

enum UiPhase {
    Hidden,
    Picker,
    Settings,
    Working { label: String },
    Popup { title: String, body: String },
    Error { message: String },
}

struct ServeApp {
    config: AppConfig,
    config_path: PathBuf,
    selection: Arc<MacosSelection>,
    hotkey: MacosHotkey,
    config_mtime: Option<SystemTime>,
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
    job_rx: Receiver<JobResult>,
    job_tx: Sender<JobResult>,
    runtime: tokio::runtime::Runtime,
    status_line: String,
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
        hotkey.set_wake(move || {
            egui_ctx.request_repaint();
        });
        // Must register on the main thread (eframe creation runs there).
        Self::register_hotkeys(&hotkey, &config)?;
        let config_mtime = std::fs::metadata(&config_path)
            .and_then(|m| m.modified())
            .ok();

        let (job_tx, job_rx) = mpsc::channel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("tokio runtime")?;

        Ok(Self {
            status_line: Self::status_for(&config),
            config,
            config_path,
            selection,
            hotkey,
            config_mtime,
            phase: UiPhase::Hidden,
            captured_text: String::new(),
            captured_app: None,
            captured_range: None,
            target_pid: None,
            soft_warn_acked: false,
            replace_warn_acked: false,
            pending_direct: None,
            settings_status: String::new(),
            job_rx,
            job_tx,
            runtime,
        })
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
        format!(
            "Picker: {} · {} cmds · {} · Access: {}",
            config.hotkey,
            config.commands.len(),
            if cmd_hk == 0 {
                shortcut_hint
            } else if cmd_hk == 1 {
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
        eprintln!(
            "selara: registering picker `{}` + {} command shortcut(s)",
            config.hotkey,
            cmd_keys.len()
        );
        for (id, spec) in &cmd_keys {
            eprintln!("selara:   command `{id}` → `{spec}`");
        }
        hotkey
            .reregister_all(&config.hotkey, &cmd_keys)
            .with_context(|| format!("register hotkeys (picker `{}`)", config.hotkey))?;
        Ok(())
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
        self.target_pid = frontmost_pid();
        self.soft_warn_acked = false;
        self.replace_warn_acked = false;
        self.pending_direct = None;
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

    fn on_hotkey(&mut self, ctx: &egui::Context) {
        if !accessibility_trusted() {
            prompt_accessibility();
            self.phase = UiPhase::Error {
                message: "Accessibility permission missing.\n\n\
System Settings → Privacy & Security → Accessibility\n\
Enable Selara (or Terminal / the binary you launched),\n\
then restart `selara serve`."
                    .into(),
            };
            self.show_window(ctx, true);
            return;
        }

        match self.capture_selection() {
            Ok(true) => {
                self.phase = UiPhase::Picker;
                self.show_window(ctx, true);
            }
            Ok(false) => {}
            Err(message) => {
                self.phase = UiPhase::Error { message };
                self.show_window(ctx, true);
            }
        }
    }

    fn on_command_hotkey(&mut self, ctx: &egui::Context, command_id: &str) {
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
            self.show_window(ctx, true);
            return;
        };
        match self.capture_selection() {
            Ok(true) => {
                self.show_window(ctx, true);
                if self.needs_confirmation(&cmd) {
                    // Same rails as the picker: show it with the banner and run
                    // the command once the user confirms.
                    eprintln!(
                        "selara: command hotkey `{}` → waiting for confirmation",
                        cmd.id
                    );
                    self.pending_direct = Some(cmd);
                    self.phase = UiPhase::Picker;
                } else {
                    eprintln!("selara: command hotkey `{}` → running", cmd.id);
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
                self.show_window(ctx, true);
            }
            Err(message) => {
                self.phase = UiPhase::Error { message };
                self.show_window(ctx, true);
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

    fn start_command(&mut self, cmd: WritingCommand) {
        // Picking a command by hand supersedes any shortcut-triggered one.
        self.pending_direct = None;
        if self.over_hard_max() {
            let max = self.config.limits.hard_max_chars;
            self.phase = UiPhase::Error {
                message: format!(
                    "Selection is {} characters — over your hard limit of {max}.\n\n\
Shrink the selection, or raise / disable the limit in Settings (0 = unlimited).",
                    self.selection_chars()
                ),
            };
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
        self.phase = UiPhase::Working {
            label: label.clone(),
        };

        self.runtime.spawn(async move {
            let result = async {
                let provider = cfg.build_provider()?;
                let out =
                    run_command(provider.as_ref(), &cmd, &input, None, Some(&cfg.language)).await?;
                Ok::<_, anyhow::Error>((cmd.kind, cmd.label, out))
            }
            .await;

            let msg = match result {
                Ok((kind, label, text)) => JobResult::Success { kind, label, text },
                Err(e) => JobResult::Error(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    fn apply_job(&mut self, ctx: &egui::Context, job: JobResult) {
        match job {
            JobResult::Error(message) => {
                self.phase = UiPhase::Error { message };
            }
            JobResult::Success { kind, label, text } => match kind {
                CommandKind::Popup => {
                    self.phase = UiPhase::Popup {
                        title: label,
                        body: text,
                    };
                }
                CommandKind::Replace => {
                    // Hide first so macOS can restore focus to the source app,
                    // then activate + paste. Pasting while we are still frontmost fails.
                    let pid = self.target_pid;
                    let original = self.captured_text.clone();
                    let range = self.captured_range;
                    self.hide(ctx);
                    std::thread::sleep(std::time::Duration::from_millis(80));
                    if let Some(pid) = pid {
                        let _ = activate_pid(pid);
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(180));
                    }
                    match self.selection.replace_in_app(pid, &text, &original, range) {
                        Ok(()) => {}
                        Err(e) => {
                            self.phase = UiPhase::Error {
                                message: format!("Replace failed: {e}"),
                            };
                            self.show_window(ctx, true);
                        }
                    }
                }
            },
        }
    }
}

fn block_on_ready<T>(fut: impl std::future::Future<Output = T>) -> T {
    use std::task::{Context as TaskCtx, Poll, RawWaker, RawWakerVTable, Waker};

    fn noop_raw_waker() -> RawWaker {
        fn no_op(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            noop_raw_waker()
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
        RawWaker::new(std::ptr::null(), &VTABLE)
    }

    let waker = unsafe { Waker::from_raw(noop_raw_waker()) };
    let mut cx = TaskCtx::from_waker(&waker);
    let mut fut = Box::pin(fut);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(fut),
    }
}

impl eframe::App for ServeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.hotkey.poll();
        self.maybe_reload_config();

        if let Some(action) = self.hotkey.take_pending() {
            match action {
                HotkeyAction::Picker => self.on_hotkey(ctx),
                HotkeyAction::Command(id) => self.on_command_hotkey(ctx, &id),
            }
        }

        while let Ok(job) = self.job_rx.try_recv() {
            self.apply_job(ctx, job);
        }

        if matches!(self.phase, UiPhase::Working { .. }) {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        if matches!(self.phase, UiPhase::Hidden) {
            // Keep the event loop alive so hotkey.poll / wake still run after hide.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            egui::CentralPanel::default().show(ctx, |_ui| {});
            return;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if matches!(self.phase, UiPhase::Settings) {
                self.phase = UiPhase::Picker;
            } else {
                self.hide(ctx);
            }
            return;
        }

        // Collect click target without holding a borrow across mutation.
        let mut clicked: Option<WritingCommand> = None;
        let mut dismiss = false;
        let mut open_settings = false;
        let mut back_to_picker = false;
        let mut save_settings = false;
        let mut reset_limits = false;
        let mut ack_soft = false;
        let mut ack_replace = false;

        let soft_blocked = matches!(self.phase, UiPhase::Picker) && self.needs_soft_warn();
        let hard_blocked = matches!(self.phase, UiPhase::Picker) && self.over_hard_max();
        let replace_caution = matches!(self.phase, UiPhase::Picker) && self.needs_replace_warn();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Selara");
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
                    let preview: String = self.captured_text.chars().take(220).collect();
                    ui.small(if self.captured_text.chars().count() > 220 {
                        format!("{preview}…")
                    } else {
                        preview
                    });

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
                    ui.label("Choose a command:");

                    let commands = self.config.commands.clone();
                    for cmd in commands {
                        let kind_tag = match cmd.kind {
                            CommandKind::Replace => "replace",
                            CommandKind::Popup => "popup",
                        };
                        let replace_locked = matches!(cmd.kind, CommandKind::Replace)
                            && replace_caution
                            && !hard_blocked
                            && !soft_blocked;
                        let enabled = !hard_blocked && !soft_blocked && !replace_locked;
                        let resp = ui.add_enabled(
                            enabled,
                            egui::Button::new(format!("{}  ({kind_tag})", cmd.label))
                                .min_size(egui::vec2(ui.available_width(), 28.0)),
                        );
                        if resp.clicked() {
                            clicked = Some(cmd);
                        }
                    }
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
                UiPhase::Working { label } => {
                    ui.label(format!("Running {label}…"));
                    ui.spinner();
                }
                UiPhase::Popup { title, body } => {
                    ui.heading(title);
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        ui.label(body);
                    });
                    if ui.button("Copy result").clicked() {
                        ui.ctx().copy_text(body.clone());
                    }
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
        }
        if reset_limits {
            self.config.limits = LimitsConfig::default();
            self.settings_status = "Defaults restored (not saved yet)".into();
        }
        if save_settings {
            self.save_settings();
        }
        if dismiss {
            self.hide(ctx);
        }
        if let Some(cmd) = clicked {
            self.start_command(cmd);
        }
        if matches!(self.phase, UiPhase::Picker) {
            self.run_pending_if_ready();
        }
    }
}

pub fn run(config_path: PathBuf) -> Result<()> {
    let config = AppConfig::load_or_init(&config_path)?;
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
            .with_inner_size([400.0, 520.0])
            .with_min_inner_size([320.0, 400.0])
            .with_resizable(true)
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
