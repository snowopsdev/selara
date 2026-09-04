//! macOS desktop shell: global hotkey → command picker → replace / popup.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use anyhow::{Context, Result};
use eframe::egui;
use writing_tools_core::commands::{run_command, CommandKind, WritingCommand};
use writing_tools_core::config::AppConfig;
use writing_tools_core::providers::provider_from_config;
use writing_tools_platform::macos::{
    accessibility_trusted, activate_pid, frontmost_pid, prompt_accessibility, MacosHotkey,
    MacosSelection,
};
use writing_tools_platform::{HotkeyService, SelectionService};

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
    Working { label: String },
    Popup { title: String, body: String },
    Error { message: String },
}

struct ServeApp {
    config: AppConfig,
    selection: Arc<MacosSelection>,
    hotkey: MacosHotkey,
    hotkey_fired: Arc<AtomicBool>,
    phase: UiPhase,
    /// Text captured at hotkey time (before our window steals focus).
    captured_text: String,
    captured_app: Option<String>,
    target_pid: Option<i32>,
    job_rx: Receiver<JobResult>,
    job_tx: Sender<JobResult>,
    runtime: tokio::runtime::Runtime,
    status_line: String,
}

impl ServeApp {
    fn new(
        _cc: &eframe::CreationContext<'_>,
        config: AppConfig,
        selection: Arc<MacosSelection>,
    ) -> Result<Self> {
        let hotkey_fired = Arc::new(AtomicBool::new(false));
        let hotkey = MacosHotkey::new();
        let flag = hotkey_fired.clone();
        // Wake egui when the hotkey fires so a hidden window still updates.
        let egui_ctx = _cc.egui_ctx.clone();
        hotkey.set_wake(move || {
            egui_ctx.request_repaint();
        });
        // Must register on the main thread (eframe creation runs there).
        block_on_ready(hotkey.register(
            &config.hotkey,
            Box::new(move || {
                flag.store(true, Ordering::SeqCst);
            }),
        ))
        .with_context(|| format!("register hotkey `{}`", config.hotkey))?;

        let (job_tx, job_rx) = mpsc::channel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("tokio runtime")?;

        Ok(Self {
            status_line: format!(
                "Hotkey: {} · {} commands · Accessibility: {}",
                config.hotkey,
                config.commands.len(),
                if accessibility_trusted() {
                    "granted"
                } else {
                    "MISSING"
                }
            ),
            config,
            selection,
            hotkey,
            hotkey_fired,
            phase: UiPhase::Hidden,
            captured_text: String::new(),
            captured_app: None,
            target_pid: None,
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

    fn on_hotkey(&mut self, ctx: &egui::Context) {
        if !accessibility_trusted() {
            prompt_accessibility();
            self.phase = UiPhase::Error {
                message: "Accessibility permission missing.\n\n\
System Settings → Privacy & Security → Accessibility\n\
Enable Writing Tools (or Terminal / the binary you launched),\n\
then restart `writing-tools serve`."
                    .into(),
            };
            self.show_window(ctx, true);
            return;
        }

        // Capture focus target + selection BEFORE our window activates.
        self.target_pid = frontmost_pid();
        match self.runtime.block_on(self.selection.read_selection()) {
            Ok(Some(snap)) => {
                self.captured_text = snap.text;
                self.captured_app = snap.app_name;
            }
            Ok(None) => {
                self.phase = UiPhase::Error {
                    message: "No text selection found.\nSelect text in another app, then press the hotkey again.".into(),
                };
                self.show_window(ctx, true);
                return;
            }
            Err(e) => {
                self.phase = UiPhase::Error {
                    message: format!("{e}"),
                };
                self.show_window(ctx, true);
                return;
            }
        }

        self.phase = UiPhase::Picker;
        self.show_window(ctx, true);
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.phase = UiPhase::Hidden;
        self.show_window(ctx, false);
    }

    fn start_command(&mut self, cmd: WritingCommand) {
        let input = self.captured_text.clone();
        let cfg = self.config.clone();
        let tx = self.job_tx.clone();
        let label = cmd.label.clone();
        self.phase = UiPhase::Working {
            label: label.clone(),
        };

        self.runtime.spawn(async move {
            let result = async {
                let api_key = cfg.resolve_api_key()?;
                let provider = provider_from_config(
                    cfg.provider.kind,
                    &cfg.provider.base_url,
                    &cfg.provider.model,
                    &api_key,
                );
                let out = run_command(provider.as_ref(), &cmd, &input, None).await?;
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
                    if let Some(pid) = self.target_pid {
                        let _ = activate_pid(pid);
                    }
                    match self
                        .runtime
                        .block_on(self.selection.replace_selection(&text))
                    {
                        Ok(()) => self.hide(ctx),
                        Err(e) => {
                            self.phase = UiPhase::Error {
                                message: format!("Replace failed: {e}"),
                            };
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

        if self.hotkey_fired.swap(false, Ordering::SeqCst) {
            self.on_hotkey(ctx);
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
            self.hide(ctx);
            return;
        }

        // Collect click target without holding a borrow across mutation.
        let mut clicked: Option<WritingCommand> = None;
        let mut dismiss = false;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Writing Tools");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        dismiss = true;
                    }
                });
            });
            ui.label(&self.status_line);
            if let Some(app) = &self.captured_app {
                ui.label(format!("From: {app}"));
            }
            ui.separator();

            match &self.phase {
                UiPhase::Picker => {
                    ui.label(format!(
                        "Selection ({} chars)",
                        self.captured_text.chars().count()
                    ));
                    let preview: String = self.captured_text.chars().take(220).collect();
                    ui.small(if self.captured_text.chars().count() > 220 {
                        format!("{preview}…")
                    } else {
                        preview
                    });
                    ui.add_space(8.0);
                    ui.label("Choose a command:");

                    for cmd in &self.config.commands {
                        let kind_tag = match cmd.kind {
                            CommandKind::Replace => "replace",
                            CommandKind::Popup => "popup",
                        };
                        if ui
                            .add_sized(
                                [ui.available_width(), 28.0],
                                egui::Button::new(format!("{}  ({kind_tag})", cmd.label)),
                            )
                            .clicked()
                        {
                            clicked = Some(cmd.clone());
                        }
                    }
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
                    if ui.button("Dismiss").clicked() {
                        dismiss = true;
                    }
                }
                UiPhase::Hidden => {}
            }
        });

        if dismiss {
            self.hide(ctx);
        }
        if let Some(cmd) = clicked {
            self.start_command(cmd);
        }
    }
}

pub fn run(config_path: PathBuf) -> Result<()> {
    let config = AppConfig::load_or_init(&config_path)?;
    println!("config: {}", config_path.display());
    println!("hotkey: {}", config.hotkey);
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([360.0, 480.0])
            .with_min_inner_size([300.0, 360.0])
            .with_resizable(true)
            .with_always_on_top()
            .with_visible(false)
            .with_title("Writing Tools"),
        ..Default::default()
    };

    eframe::run_native(
        "Writing Tools",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ServeApp::new(cc, config, selection)?) as Box<dyn eframe::App>)
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))?;

    Ok(())
}
