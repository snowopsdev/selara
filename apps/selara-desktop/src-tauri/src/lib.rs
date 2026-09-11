mod update_backup;

use selara_core::app_server;
use selara_core::codex_cli::{self, CodexLoginStatus};
use selara_core::commands::{
    merge_commands, parse_command_pack, render_command_pack, MergeMode, MergeReport,
};
use selara_core::config::{serve_pidfile, ApiKeySource, AppConfig};
use selara_core::history::{self, HistoryEntry};
use selara_core::providers::{list_provider_models, ProviderKind};
use selara_core::secrets;
use selara_core::usage::{self, UsageSummary};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, Runtime, WindowEvent, Wry,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_shell::process::{CommandChild, CommandEvent, TerminatedPayload};
use tauri_plugin_shell::ShellExt;
use tauri_plugin_updater::{Update, UpdaterExt};

/// Serializes every read-modify-write of config.toml in this process.
///
/// `save_config_section` re-reads the file, edits one section, and writes the
/// whole config back. Without a lock two of those transactions interleave —
/// both read the same starting file, and the second write lands on top of the
/// first, silently dropping the section it saved. Holding this for the whole
/// read → edit → write turns each save into one transaction. Every command that
/// writes the file takes it, including whole-config saves, so a section save
/// cannot interleave with one of those either.
///
/// Cross-process writers (`selara serve` editing Limits) are outside its reach;
/// the re-read narrows that window to the write itself, and closing it fully
/// would need file locking.
static CONFIG_WRITE: Mutex<()> = Mutex::new(());
static APP_OPERATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static MAINTENANCE: AtomicBool = AtomicBool::new(false);
struct LoginState {
    active: bool,
    cancelled: bool,
}
static LOGIN: Mutex<LoginState> = Mutex::new(LoginState {
    active: false,
    cancelled: false,
});
struct LoginAttempt;
impl Drop for LoginAttempt {
    fn drop(&mut self) {
        lock(&LOGIN).active = false;
    }
}
struct Maintenance;
impl Maintenance {
    fn begin() -> Self {
        MAINTENANCE.store(true, Ordering::SeqCst);
        Self
    }
}
impl Drop for Maintenance {
    fn drop(&mut self) {
        MAINTENANCE.store(false, Ordering::SeqCst);
    }
}

/// A poisoned lock only means a panic elsewhere; the file is still consistent
/// because a panicking save leaves the previous contents in place.
fn config_write_lock() -> MutexGuard<'static, ()> {
    CONFIG_WRITE.lock().unwrap_or_else(|e| e.into_inner())
}

#[tauri::command]
fn get_config() -> Result<AppConfig, String> {
    let path = AppConfig::default_path();
    AppConfig::load_or_init(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_config(config: AppConfig) -> Result<(), String> {
    let _tx = config_write_lock();
    let path = AppConfig::default_path();
    config.save(&path).map_err(|e| e.to_string())
}

/// Save one Settings tab without touching the others. The file is re-read
/// first so a change made elsewhere (for example the Limits page inside
/// `selara serve`) survives, and the merged config is returned so the UI can
/// refresh its copy.
#[tauri::command]
fn save_config_section(section: String, value: serde_json::Value) -> Result<AppConfig, String> {
    // Held across the read, the edit and the write: this is one transaction.
    let _tx = config_write_lock();
    let path = AppConfig::default_path();
    let mut cfg = AppConfig::load_or_init(&path).map_err(|e| e.to_string())?;
    cfg.apply_section(&section, value)
        .map_err(|e| e.to_string())?;
    cfg.save(&path).map_err(|e| e.to_string())?;
    Ok(cfg)
}

/// Where the current provider's key comes from (env, keychain, config, none).
#[tauri::command]
fn api_key_source() -> Result<ApiKeySource, String> {
    let cfg = AppConfig::load_or_init(&AppConfig::default_path()).map_err(|e| e.to_string())?;
    Ok(cfg.api_key_source())
}

/// Store a key in the OS keychain for `kind` and drop any plaintext copy from
/// config.toml so the keychain entry is what `serve` and the CLI use.
#[tauri::command]
fn store_api_key(kind: ProviderKind, api_key: String) -> Result<ApiKeySource, String> {
    secrets::keychain_set(kind, &api_key).map_err(|e| e.to_string())?;
    let path = AppConfig::default_path();
    let mut cfg = AppConfig::load_or_init(&path).map_err(|e| e.to_string())?;
    if cfg.provider.kind == kind && cfg.provider.api_key.is_some() {
        cfg.provider.api_key = None;
        cfg.save(&path).map_err(|e| e.to_string())?;
    }
    Ok(cfg.api_key_source())
}

#[tauri::command]
fn clear_api_key(kind: ProviderKind) -> Result<ApiKeySource, String> {
    secrets::keychain_delete(kind).map_err(|e| e.to_string())?;
    let cfg = AppConfig::load_or_init(&AppConfig::default_path()).map_err(|e| e.to_string())?;
    Ok(cfg.api_key_source())
}

/// Save every command as a TOML pack via a save dialog. Returns the path, or
/// `None` when the dialog was cancelled. Runs off the main thread because the
/// blocking dialog must not be shown from it.
#[tauri::command]
async fn export_commands(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let cfg = AppConfig::load_or_init(&AppConfig::default_path()).map_err(|e| e.to_string())?;
    let text = render_command_pack(&cfg.commands).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = app
            .dialog()
            .file()
            .set_title("Export Selara commands")
            .set_file_name("selara-commands.toml")
            .add_filter("Command pack", &["toml", "json"])
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = picked.into_path().map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(Some(path.display().to_string()))
    })
    .await
    .map_err(|e| format!("export task failed: {e}"))?
}

/// Pick a TOML/JSON pack, merge it into the config with `mode`, save, and
/// report what happened. `None` when the dialog was cancelled.
#[tauri::command]
async fn import_commands(
    app: tauri::AppHandle,
    mode: MergeMode,
) -> Result<Option<MergeReport>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = app
            .dialog()
            .file()
            .set_title("Import Selara commands")
            .add_filter("Command pack", &["toml", "json"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = picked.into_path().map_err(|e| e.to_string())?;
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let incoming = parse_command_pack(&text).map_err(|e| e.to_string())?;
        let cfg_path = AppConfig::default_path();
        let mut cfg = AppConfig::load_or_init(&cfg_path).map_err(|e| e.to_string())?;
        // The picker and undo hotkeys are registered before any command
        // hotkey, so an import that claims one of them would make the next
        // `serve` config reload fail instead of activating.
        let picker = cfg.hotkey.clone();
        let undo = cfg.undo_hotkey.clone().unwrap_or_default();
        let mut reserved: Vec<&str> = vec![picker.as_str()];
        if !undo.trim().is_empty() {
            reserved.push(undo.as_str());
        }
        let report = merge_commands(&mut cfg.commands, incoming, mode, &reserved);
        cfg.save(&cfg_path).map_err(|e| e.to_string())?;
        Ok(Some(report))
    })
    .await
    .map_err(|e| format!("import task failed: {e}"))?
}

#[tauri::command]
fn config_path() -> String {
    AppConfig::default_path().display().to_string()
}

/// Transformations `selara serve` recorded, newest first (see `selara_core::history`).
#[tauri::command]
fn history_list() -> Result<Vec<HistoryEntry>, String> {
    history::load(&history::history_path(&AppConfig::default_path())).map_err(|e| e.to_string())
}

#[tauri::command]
fn history_clear() -> Result<(), String> {
    history::clear(&history::history_path(&AppConfig::default_path())).map_err(|e| e.to_string())
}

#[tauri::command]
fn history_path() -> String {
    history::history_path(&AppConfig::default_path())
        .display()
        .to_string()
}

fn configure_auth_home() -> Result<(), String> {
    let config = AppConfig::load_or_init(&AppConfig::default_path()).map_err(|e| e.to_string())?;
    app_server::configure_home(config.provider.codex_home.as_deref());
    Ok(())
}

#[tauri::command]
async fn chatgpt_auth_status() -> Result<CodexLoginStatus, String> {
    if MAINTENANCE.load(Ordering::SeqCst) {
        return Err("An account change or update is in progress".into());
    }
    configure_auth_home()?;
    codex_cli::login_status().await.map_err(|e| e.to_string())
}

async fn change_auth<F, Fut>(app: AppHandle, operation: F) -> Result<CodexLoginStatus, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let _operation = APP_OPERATION
        .try_lock()
        .map_err(|_| "An account change or update is already in progress")?;
    let _maintenance = Maintenance::begin();
    configure_auth_home()?;
    let handle = app.clone();
    let was_running = tauri::async_runtime::spawn_blocking(move || {
        let sup = handle.state::<Supervisor>(); let _transition = lock(&sup.transition);
        if sup.managed_pid().is_none() {
            if serve_status().running { return Err("Stop the externally started background service before changing the shared Codex account".to_string()); }
            return Ok(false);
        }
        if let Err(error) = quiesce_serve_locked(&handle, &sup) {
            let _ = resume_serve_locked(&handle, &sup); return Err(error);
        }
        Ok(true)
    }).await.map_err(|e| e.to_string())??;
    let outcome = operation().await;
    let _ = app_server::reset().await;
    let handle = app.clone();
    let resumed = tauri::async_runtime::spawn_blocking(move || {
        let sup = handle.state::<Supervisor>();
        let _transition = lock(&sup.transition);
        if was_running && sup.managed_pid().is_some() {
            let reload = protocol_request_locked(
                &handle,
                &sup,
                selara_core::desktop_protocol::ProtocolCommand::ReloadAuth,
            );
            let resume = resume_serve_locked(&handle, &sup);
            reload?;
            resume?;
        } else if was_running && sup.desired.load(Ordering::SeqCst) {
            start_serve_locked(&handle, &sup)?;
        }
        Ok::<_, String>(())
    })
    .await
    .map_err(|e| e.to_string())?;
    outcome?;
    resumed?;
    codex_cli::login_status().await.map_err(|e| e.to_string())
}

#[tauri::command]
#[allow(deprecated)]
async fn chatgpt_login(app: AppHandle) -> Result<CodexLoginStatus, String> {
    {
        let mut login = lock(&LOGIN);
        if login.active {
            return Err("Sign-in is already in progress".into());
        }
        *login = LoginState {
            active: true,
            cancelled: false,
        };
    }
    let _attempt = LoginAttempt;
    let handle = app.clone();
    change_auth(app, || async move {
        if lock(&LOGIN).cancelled {
            return Err("Sign-in cancelled".into());
        }
        let login = app_server::begin_login().await.map_err(|e| e.to_string())?;
        if lock(&LOGIN).cancelled {
            let _ = app_server::cancel_login().await;
            return Err("Sign-in cancelled".into());
        }
        if let Err(error) = handle.shell().open(&login.auth_url, None) {
            let _ = app_server::cancel_login().await;
            return Err(error.to_string());
        }
        login.wait().await.map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn chatgpt_login_cancel() -> Result<(), String> {
    {
        let mut login = lock(&LOGIN);
        if !login.active {
            return Err("No sign-in is in progress".into());
        }
        login.cancelled = true;
    }
    match app_server::cancel_login().await {
        Ok(()) => Ok(()),
        Err(error)
            if error
                .to_string()
                .contains("No browser sign-in is in progress") =>
        {
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
async fn chatgpt_logout(app: AppHandle) -> Result<CodexLoginStatus, String> {
    change_auth(app, || async {
        app_server::logout().await.map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn list_chatgpt_models_cmd() -> Result<Vec<String>, String> {
    if MAINTENANCE.load(Ordering::SeqCst) {
        return Err("An account change or update is in progress".into());
    }
    configure_auth_home()?;
    app_server::models().await.map_err(|e| e.to_string())
}

/// List models for a BYOK provider. Falls back to the env API key when the
/// Settings form has no key typed in yet.
#[tauri::command]
async fn list_provider_models_cmd(
    kind: ProviderKind,
    base_url: String,
    api_key: Option<String>,
) -> Result<Vec<String>, String> {
    let key = api_key
        .filter(|k| !k.trim().is_empty())
        .or_else(|| {
            ["SELARA_API_KEY", "WRITING_TOOLS_API_KEY"]
                .iter()
                .find_map(|var| std::env::var(var).ok())
                .filter(|k| !k.trim().is_empty())
        })
        .or_else(|| secrets::keychain_get(kind).ok().flatten())
        .unwrap_or_default();
    list_provider_models(kind, &base_url, key.trim())
        .await
        .map_err(|e| e.to_string())
}

/// Whether `selara serve` is running, read from the `serve.pid` file it writes
/// next to the config. Shown on the Settings app's Status tab.
#[derive(Debug, Clone, serde::Serialize)]
struct ServeStatus {
    running: bool,
    pid: Option<u32>,
    pidfile: String,
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Signal 0 checks existence without delivering anything.
        pid > 0 && unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Token totals and estimated cost from the local `usage.jsonl` ledger.
#[tauri::command]
fn usage_summary() -> Result<UsageSummary, String> {
    let path = usage::usage_path(&AppConfig::default_path());
    usage::summary(&path).map_err(|e| e.to_string())
}

/// Truncate the local usage ledger and return the (now empty) summary.
#[tauri::command]
fn clear_usage() -> Result<UsageSummary, String> {
    let path = usage::usage_path(&AppConfig::default_path());
    usage::clear(&path).map_err(|e| e.to_string())?;
    usage::summary(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn serve_status() -> ServeStatus {
    let pidfile = serve_pidfile(&AppConfig::default_path());
    let pid = std::fs::read_to_string(&pidfile)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok());
    ServeStatus {
        running: pid.is_some_and(pid_alive),
        pid,
        pidfile: pidfile.display().to_string(),
    }
}

/// Only the managed selection process can report its Accessibility grant.
#[tauri::command]
fn accessibility_status(app: AppHandle) -> selara_core::desktop_protocol::AxTrust {
    use selara_core::desktop_protocol::AxTrust;
    let sup = app.state::<Supervisor>();
    if sup.managed_pid().is_none() {
        return AxTrust::Unknown;
    }
    let trust = lock(&sup.protocol_status)
        .as_ref()
        .map(|s| s.ax_trust)
        .unwrap_or(AxTrust::Unknown);
    trust
}

#[tauri::command]
#[allow(deprecated)]
async fn open_accessibility_settings(app: AppHandle) -> Result<(), String> {
    if app.state::<Supervisor>().managed_pid().is_some() {
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let sup = handle.state::<Supervisor>();
            let _transition = lock(&sup.transition);
            protocol_request_locked(
                &handle,
                &sup,
                selara_core::desktop_protocol::ProtocolCommand::RequestPermission,
            )
            .map(|_| ())
        })
        .await
        .map_err(|e| e.to_string())??;
    }
    app.shell()
        .open(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            None,
        )
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// serve supervisor
//
// The Settings app bundles the `selara` CLI as a Tauri sidecar and runs
// `selara serve` itself, so a user no longer needs a terminal open for the
// hotkey. A `serve` started by hand (for example `cargo run -p selara --
// serve`) is detected through its pidfile and left alone.
// ---------------------------------------------------------------------------

/// Lines of `serve` output kept for the Status tab's log panel.
const LOG_LINES: usize = 200;
/// Give up restarting when `serve` exits this many times within the window.
const MAX_RESTARTS: usize = 5;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
/// How long `stop_serve` waits for SIGTERM before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Supervisor {
    request_sequence: AtomicU64,
    /// Serializes every start / stop / restart.
    ///
    /// The checks that decide whether to spawn read `child` and the pidfile,
    /// and both locks are released again before the new handle is stored. Two
    /// overlapping requests — the `RunEvent::Ready` start racing a tray or
    /// Status-tab click, say — could therefore both pass those checks, spawn a
    /// sidecar each, and leave the second `child` assignment hiding the first
    /// process from Stop and Quit, still holding the global hotkey. This is
    /// held across the whole check → spawn → store sequence, and across a
    /// restart's stop-then-start, so only one transition is ever in flight.
    transition: Mutex<()>,
    /// Short state publications only; readers never take transition while a request waits.
    lifecycle: Mutex<()>,
    /// The sidecar we spawned, if any. `None` while stopped or external.
    child: Mutex<Option<CommandChild>>,
    /// When the last few automatic restarts happened (crash-loop guard).
    restarts: Mutex<VecDeque<Instant>>,
    /// Last `LOG_LINES` lines of stdout/stderr plus supervisor notes.
    log: Mutex<VecDeque<String>>,
    /// Whether the user wants `serve` running; a `Terminated` event only
    /// restarts while this is true.
    desired: AtomicBool,
    /// Bumped on every spawn and stop so a reader task for an old child
    /// cannot restart over a newer one.
    generation: AtomicU64,
    /// Why the last start or restart did not happen, for the UI.
    last_error: Mutex<Option<String>>,
    /// Latest status reported by a managed child. Parent Accessibility state
    /// is intentionally never substituted for this value.
    protocol_status: Mutex<Option<selara_core::desktop_protocol::ServeStatus>>,
    protocol_waiters: Mutex<
        HashMap<String, std::sync::mpsc::Sender<selara_core::desktop_protocol::ProtocolResponse>>,
    >,
}

/// A poisoned lock only means a panic elsewhere; the data is still usable.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Supervisor {
    fn push_log(&self, line: impl Into<String>) {
        let line = line.into();
        let mut log = lock(&self.log);
        if log.len() >= LOG_LINES {
            log.pop_front();
        }
        log.push_back(line);
    }

    fn set_error(&self, err: Option<String>) {
        *lock(&self.last_error) = err;
    }

    fn managed_pid(&self) -> Option<u32> {
        lock(&self.child).as_ref().map(|c| c.pid())
    }
}

/// Tray items whose enabled/checked state follows the supervisor.
struct TrayMenu {
    start: MenuItem<Wry>,
    stop: MenuItem<Wry>,
    restart: MenuItem<Wry>,
    login: CheckMenuItem<Wry>,
    /// "Update to vX.Y.Z…"; disabled with a status text until a check finds one.
    update: MenuItem<Wry>,
}

/// Re-sync the tray items and tell the Settings window something changed.
fn notify(app: &AppHandle) {
    // Menu setters wait for the main thread. A worker must never wait here
    // while holding a supervisor lock that the main thread needs during exit.
    // Read the current state when the queued publication runs so an obsolete
    // notification cannot restore an earlier process's tray state.
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(menu) = handle.try_state::<TrayMenu>() {
            let managed = handle.state::<Supervisor>().managed_pid().is_some();
            let external = !managed && serve_status().running;
            let _ = menu.start.set_enabled(!managed && !external);
            let _ = menu.stop.set_enabled(managed);
            let _ = menu.restart.set_enabled(managed);
            let _ = menu
                .login
                .set_checked(handle.autolaunch().is_enabled().unwrap_or(false));
        }
        let _ = handle.emit("serve-changed", ());
    });
}

/// Spawn the `serve` sidecar unless one (ours or external) already runs.
fn start_serve(app: &AppHandle) -> Result<(), String> {
    let sup = app.state::<Supervisor>();
    let _transition = lock(&sup.transition);
    if MAINTENANCE.load(Ordering::SeqCst) {
        return Err("Finish the account change or update before starting the service".into());
    }
    start_serve_locked(app, &sup)
}

/// The body of `start_serve`; the caller holds `Supervisor::transition`.
///
/// `notify` only takes `child` and the tray state, never `transition`, so it is
/// safe to call from here.
fn start_serve_locked(app: &AppHandle, sup: &Supervisor) -> Result<(), String> {
    if sup.managed_pid().is_some() {
        return Ok(());
    }
    let status = serve_status();
    if status.running {
        let pid = status.pid.unwrap_or(0);
        sup.push_log(format!(
            "external serve running (pid {pid}); not starting a second instance"
        ));
        sup.desired.store(false, Ordering::SeqCst);
        sup.set_error(None);
        notify(app);
        return Ok(());
    }

    let spawned = app.shell().sidecar("selara").and_then(|cmd| {
        cmd.args(["serve", "--desktop-protocol"])
            .set_raw_out(true)
            .spawn()
    });
    let (mut rx, child) = match spawned {
        Ok(pair) => pair,
        Err(e) => {
            let msg = format!("could not start serve: {e}");
            sup.push_log(msg.clone());
            sup.set_error(Some(msg.clone()));
            sup.desired.store(false, Ordering::SeqCst);
            notify(app);
            return Err(msg);
        }
    };
    let pid = child.pid();
    let generation_guard = lock(&sup.lifecycle);
    let generation = sup.generation.fetch_add(1, Ordering::SeqCst) + 1;
    *lock(&sup.protocol_status) = None;
    lock(&sup.protocol_waiters).clear();
    sup.desired.store(true, Ordering::SeqCst);
    sup.set_error(None);
    *lock(&sup.child) = Some(child);
    sup.push_log(format!("started serve (pid {pid})"));
    notify(app);
    drop(generation_guard);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut decoder = selara_core::desktop_protocol::ResponseDecoder::default();
        while let Some(event) = rx.recv().await {
            let sup = app.state::<Supervisor>();
            if sup.generation.load(Ordering::SeqCst) != generation {
                break;
            }
            match event {
                CommandEvent::Stdout(bytes) => {
                    for frame in decoder.push(&bytes) {
                        let _publication = lock(&sup.lifecycle);
                        if sup.generation.load(Ordering::SeqCst) != generation {
                            break;
                        }
                        match frame {
                            Ok(response)
                                if response.version
                                    == selara_core::desktop_protocol::PROTOCOL_VERSION
                                    && response.status.version
                                        == selara_core::desktop_protocol::PROTOCOL_VERSION =>
                            {
                                *lock(&sup.protocol_status) = Some(response.status.clone());
                                if let Some(id) = &response.id {
                                    if let Some(waiter) = lock(&sup.protocol_waiters).remove(id) {
                                        let _ = waiter.send(response.clone());
                                    }
                                }
                                let _ = app.emit("serve-protocol", response);
                                notify(&app);
                            }
                            Ok(_) => sup.push_log(
                                "Unsupported background service protocol; reinstall Selara",
                            ),
                            Err(error) => sup.push_log(error),
                        }
                    }
                }
                CommandEvent::Stderr(bytes) => {
                    sup.push_log(String::from_utf8_lossy(&bytes).trim_end().to_string());
                }
                CommandEvent::Error(e) => sup.push_log(format!("[supervisor] {e}")),
                CommandEvent::Terminated(payload) => {
                    on_terminated(&app, generation, payload);
                    break;
                }
                _ => {}
            }
        }
    });
    Ok(())
}

/// Caller holds transition across request and any following stop/install.
fn protocol_request_locked(
    _app: &AppHandle,
    sup: &Supervisor,
    command: selara_core::desktop_protocol::ProtocolCommand,
) -> Result<selara_core::desktop_protocol::ProtocolResponse, String> {
    let generation = sup.generation.load(Ordering::SeqCst);
    let sequence = sup.request_sequence.fetch_add(1, Ordering::SeqCst);
    let id = format!("desktop-{generation}-{sequence}");
    let request = selara_core::desktop_protocol::ProtocolRequest {
        version: 1,
        id: id.clone(),
        command,
    };
    let (tx, rx) = std::sync::mpsc::channel();
    lock(&sup.protocol_waiters).insert(id.clone(), tx);
    let result = (|| {
        {
            let mut child = lock(&sup.child);
            child
                .as_mut()
                .ok_or("The background service is not managed by this app")?
                .write(
                    format!(
                        "{}\n",
                        serde_json::to_string(&request).map_err(|e| e.to_string())?
                    )
                    .as_bytes(),
                )
                .map_err(|e| e.to_string())?;
        }
        let response = rx.recv_timeout(Duration::from_secs(30)).map_err(|_| {
            "The background service did not acknowledge the request; try restarting it".to_string()
        })?;
        if sup.generation.load(Ordering::SeqCst) != generation {
            return Err("The background service changed during the request".into());
        }
        if !response.ok {
            return Err(response
                .error
                .unwrap_or_else(|| "Background service request failed".into()));
        }
        Ok(response)
    })();
    lock(&sup.protocol_waiters).remove(&id);
    result
}

fn quiesce_serve_locked(
    app: &AppHandle,
    sup: &Supervisor,
) -> Result<selara_core::desktop_protocol::ProtocolResponse, String> {
    let response = protocol_request_locked(
        app,
        sup,
        selara_core::desktop_protocol::ProtocolCommand::Quiesce,
    )?;
    if response.status.readiness != selara_core::desktop_protocol::ServeReadiness::Quiesced {
        return Err("The background service has not finished pausing".into());
    }
    Ok(response)
}

fn quiesce_serve(
    app: &AppHandle,
) -> Result<selara_core::desktop_protocol::ProtocolResponse, String> {
    let sup = app.state::<Supervisor>();
    let _transition = lock(&sup.transition);
    quiesce_serve_locked(app, &sup)
}

fn resume_serve_locked(app: &AppHandle, sup: &Supervisor) -> Result<(), String> {
    protocol_request_locked(
        app,
        sup,
        selara_core::desktop_protocol::ProtocolCommand::Resume,
    )
    .map(|_| ())
}
fn resume_serve(app: &AppHandle) -> Result<(), String> {
    let sup = app.state::<Supervisor>();
    let _transition = lock(&sup.transition);
    if MAINTENANCE.load(Ordering::SeqCst) {
        return Err("Finish the account change or update before resuming the service".into());
    }
    resume_serve_locked(app, &sup)
}

/// Called from the reader task when our child exits. Restarts with backoff
/// while `desired`, unless it keeps dying (`MAX_RESTARTS` in `RESTART_WINDOW`).
fn on_terminated(app: &AppHandle, generation: u64, payload: TerminatedPayload) {
    let sup = app.state::<Supervisor>();
    let _publication = lock(&sup.lifecycle);
    if sup.generation.load(Ordering::SeqCst) != generation {
        // stop_serve or a newer start already took over this slot.
        return;
    }
    *lock(&sup.child) = None;
    *lock(&sup.protocol_status) = None;
    lock(&sup.protocol_waiters).clear();
    let why = match (payload.code, payload.signal) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(sig)) => format!("signal {sig}"),
        (None, None) => "unknown reason".to_string(),
    };
    sup.push_log(format!("serve exited ({why})"));
    if !sup.desired.load(Ordering::SeqCst) {
        notify(app);
        return;
    }

    let now = Instant::now();
    let attempt = {
        let mut restarts = lock(&sup.restarts);
        while restarts
            .front()
            .is_some_and(|t| now.duration_since(*t) > RESTART_WINDOW)
        {
            restarts.pop_front();
        }
        if restarts.len() >= MAX_RESTARTS {
            None
        } else {
            restarts.push_back(now);
            Some(restarts.len())
        }
    };
    match attempt {
        None => {
            sup.desired.store(false, Ordering::SeqCst);
            let msg = format!(
                "serve exited {MAX_RESTARTS} times within {}s (last: {why}); not restarting. Check the log below, then press Start.",
                RESTART_WINDOW.as_secs()
            );
            sup.push_log(msg.clone());
            sup.set_error(Some(msg));
            notify(app);
        }
        Some(n) => {
            // 1, 2, 4, 8, 16 seconds.
            let delay = Duration::from_secs(1 << (n - 1).min(4));
            sup.push_log(format!(
                "restarting serve in {}s (attempt {n} of {MAX_RESTARTS})",
                delay.as_secs()
            ));
            notify(app);
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                let sup = app.state::<Supervisor>();
                if sup.desired.load(Ordering::SeqCst)
                    && sup.generation.load(Ordering::SeqCst) == generation
                {
                    let _ = start_serve(&app);
                }
            });
        }
    }
}

/// Stop the managed `serve`. SIGTERM first so it can clean up, SIGKILL if it
/// is still around after `STOP_GRACE`. Waits until the pid is gone so a
/// following `start_serve` does not mistake the corpse for an external serve.
fn stop_serve(app: &AppHandle) -> Result<(), String> {
    let sup = app.state::<Supervisor>();
    let _transition = lock(&sup.transition);
    stop_serve_locked(app, &sup)
}

/// The body of `stop_serve`; the caller holds `Supervisor::transition`.
fn stop_serve_locked(app: &AppHandle, sup: &Supervisor) -> Result<(), String> {
    sup.desired.store(false, Ordering::SeqCst);
    let child = lock(&sup.child).take();
    let Some(child) = child else {
        notify(app);
        return Ok(());
    };
    let pid = child.pid();
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    let deadline = Instant::now() + STOP_GRACE;
    while pid_alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    if pid_alive(pid) {
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while pid_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let _publication = lock(&sup.lifecycle);
    if pid_alive(pid) {
        *lock(&sup.child) = Some(child);
        return Err(format!(
            "Could not stop serve (pid {pid}); it remains managed"
        ));
    }
    sup.generation.fetch_add(1, Ordering::SeqCst);
    *lock(&sup.protocol_status) = None;
    lock(&sup.protocol_waiters).clear();
    sup.push_log(format!("stopped serve (pid {pid})"));
    notify(app);
    Ok(())
}

fn restart_serve(app: &AppHandle) -> Result<(), String> {
    let sup = app.state::<Supervisor>();
    // One transition, so nothing can start a second sidecar in the window
    // between the stop and the start.
    let _transition = lock(&sup.transition);
    if MAINTENANCE.load(Ordering::SeqCst) {
        return Err("Finish the account change or update before restarting the service".into());
    }
    stop_serve_locked(app, &sup)?;
    lock(&sup.restarts).clear();
    start_serve_locked(app, &sup)
}

/// Run a supervisor action off the main thread: `stop_serve` can wait up to
/// three seconds, which must not freeze the Settings window or the tray.
async fn supervise<F>(app: AppHandle, f: F) -> Result<(), String>
where
    F: FnOnce(&AppHandle) -> Result<(), String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || f(&app))
        .await
        .map_err(|e| format!("supervisor task failed: {e}"))?
}

#[tauri::command]
async fn serve_start(app: AppHandle) -> Result<(), String> {
    supervise(app, start_serve).await
}

#[tauri::command]
async fn serve_stop(app: AppHandle) -> Result<(), String> {
    supervise(app, stop_serve).await
}

#[tauri::command]
async fn serve_restart(app: AppHandle) -> Result<(), String> {
    supervise(app, restart_serve).await
}

#[tauri::command]
async fn serve_quiesce(
    app: AppHandle,
) -> Result<selara_core::desktop_protocol::ProtocolResponse, String> {
    tauri::async_runtime::spawn_blocking(move || quiesce_serve(&app))
        .await
        .map_err(|e| format!("supervisor task failed: {e}"))?
}

#[tauri::command]
async fn serve_resume(app: AppHandle) -> Result<(), String> {
    supervise(app, resume_serve).await
}

#[tauri::command]
fn serve_log(app: AppHandle) -> Vec<String> {
    lock(&app.state::<Supervisor>().log)
        .iter()
        .cloned()
        .collect()
}

#[derive(Debug, Clone, serde::Serialize)]
struct SupervisorStatus {
    /// The app spawned the running `serve`.
    managed: bool,
    /// Pid of the running `serve`, managed or external.
    pid: Option<u32>,
    /// A `serve` started outside the app is running (we will not spawn one).
    external: bool,
    last_error: Option<String>,
    child_status: Option<selara_core::desktop_protocol::ServeStatus>,
}

#[tauri::command]
fn serve_supervisor_status(app: AppHandle) -> SupervisorStatus {
    let sup = app.state::<Supervisor>();
    let managed = sup.managed_pid();
    let last_error = lock(&sup.last_error).clone();
    let child_status = managed.and_then(|_| lock(&sup.protocol_status).clone());
    let status = serve_status();
    SupervisorStatus {
        managed: managed.is_some(),
        pid: managed.or(status.running.then_some(status.pid).flatten()),
        external: managed.is_none() && status.running,
        last_error,
        child_status,
    }
}

// ---------------------------------------------------------------------------
// auto-update
//
// Releases publish a signed `Selara.app.tar.gz` plus `latest.json`; the
// updater plugin compares the running version with that manifest. Until a
// maintainer generates the signing keypair, `tauri.conf.json` carries a
// placeholder public key and every check must fail closed (no network).
// ---------------------------------------------------------------------------

/// The `plugins.updater.pubkey` value shipped until a real key is generated.
const UPDATER_PUBKEY_PLACEHOLDER: &str = "REPLACE_WITH_TAURI_UPDATER_PUBKEY";
/// First automatic check after launch waits this long so startup stays quick.
const UPDATE_CHECK_STARTUP_DELAY: Duration = Duration::from_secs(10);
/// Interval between automatic checks while the app runs.
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether the configured updater public key is real. An empty key or the
/// placeholder means no release could have been signed for this build, so a
/// check would only ever fail; callers skip the network in that case.
fn updater_configured(pubkey: &str) -> bool {
    let key = pubkey.trim();
    !key.is_empty() && key != UPDATER_PUBKEY_PLACEHOLDER
}

/// `plugins.updater.pubkey` from the bundled `tauri.conf.json`.
fn configured_pubkey(app: &AppHandle) -> String {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|v| v.get("pubkey"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Result of an update check, for the Status tab and the tray.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum UpdateStatus {
    /// The build has no real signing key; checks are skipped.
    Unconfigured,
    UpToDate {
        current: String,
    },
    Available {
        current: String,
        version: String,
        notes: Option<String>,
        date: Option<String>,
    },
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Waiting {
        version: String,
    },
    Installing {
        version: String,
    },
    Error {
        message: String,
    },
}

#[derive(Default)]
struct UpdateState {
    /// The update found by the last check, ready for `install_update`.
    pending: Mutex<Option<Update>>,
    /// Last result, so the Status tab can show it without a new check.
    last: Mutex<Option<UpdateStatus>>,
    /// Set while an install runs. `download_and_install` replaces the app
    /// bundle and then asks for a restart, so two concurrent runs would race
    /// over the same files.
    installing: AtomicBool,
}

/// Push the result to the tray item and the Settings window.
fn apply_update_status(app: &AppHandle, status: &UpdateStatus) {
    if let Some(menu) = app.try_state::<TrayMenu>() {
        let (text, enabled) = match status {
            UpdateStatus::Unconfigured => ("Updates not configured".to_string(), false),
            UpdateStatus::UpToDate { current } => {
                (format!("Selara v{current} is up to date"), false)
            }
            UpdateStatus::Available { version, .. } => (format!("Update to v{version}…"), true),
            UpdateStatus::Downloading { .. } => ("Downloading update…".into(), false),
            UpdateStatus::Waiting { .. } => ("Finishing active work…".into(), false),
            UpdateStatus::Installing { .. } => ("Installing update…".into(), false),
            UpdateStatus::Error { .. } => (
                "Update failed; try again…".to_string(),
                app.state::<UpdateState>()
                    .pending
                    .lock()
                    .is_ok_and(|p| p.is_some()),
            ),
        };
        // An install in flight owns this item: a check that finishes in the
        // middle of one must not re-enable it.
        let installing = app
            .try_state::<UpdateState>()
            .is_some_and(|s| s.installing.load(Ordering::SeqCst));
        let _ = menu.update.set_text(text);
        let _ = menu.update.set_enabled(enabled && !installing);
    }
    let _ = app.emit("update-changed", status.clone());
}

/// Ask the release feed for a newer build. Never contacts the network while
/// the placeholder public key is configured.
async fn perform_update_check(app: &AppHandle) -> UpdateStatus {
    let Ok(_operation) = APP_OPERATION.try_lock() else {
        return lock(&app.state::<UpdateState>().last)
            .clone()
            .unwrap_or(UpdateStatus::Error {
                message: "An account change or update is in progress".into(),
            });
    };
    perform_update_check_inner(app).await
}

fn publish_update_status(app: &AppHandle, status: UpdateStatus) {
    *lock(&app.state::<UpdateState>().last) = Some(status.clone());
    apply_update_status(app, &status);
}

async fn perform_update_check_inner(app: &AppHandle) -> UpdateStatus {
    let current = app.package_info().version.to_string();
    let state = app.state::<UpdateState>();
    let status = if !updater_configured(&configured_pubkey(app)) {
        UpdateStatus::Unconfigured
    } else {
        let result = async {
            let updater = app
                .updater_builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| e.to_string())?;
            updater.check().await.map_err(|e| e.to_string())
        }
        .await;
        match result {
            Ok(Some(mut update)) => {
                update.timeout = Some(Duration::from_secs(300));
                let status = UpdateStatus::Available {
                    current,
                    version: update.version.clone(),
                    notes: update.body.clone(),
                    date: update.date.map(|d| d.to_string()),
                };
                *lock(&state.pending) = Some(update);
                status
            }
            Ok(None) => UpdateStatus::UpToDate { current },
            Err(message) => UpdateStatus::Error { message },
        }
    };
    if !matches!(status, UpdateStatus::Available { .. }) {
        *lock(&state.pending) = None;
    }
    *lock(&state.last) = Some(status.clone());
    apply_update_status(app, &status);
    status
}

/// Run one install at a time. The tray item and the Status button both reach
/// `install_update`, and each caller clones the same pending `Update`, so
/// without this guard two `download_and_install` runs could replace the app
/// bundle concurrently and both ask for a restart.
async fn perform_update_install(app: &AppHandle) -> Result<(), String> {
    let _operation = APP_OPERATION
        .try_lock()
        .map_err(|_| "An account change or update check is already in progress")?;
    let _maintenance = Maintenance::begin();
    if app
        .state::<UpdateState>()
        .installing
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("an update is already being installed".to_string());
    }
    if let Some(menu) = app.try_state::<TrayMenu>() {
        let _ = menu.update.set_text("Installing update…");
        let _ = menu.update.set_enabled(false);
    }
    let result = install_pending_update(app).await;
    if let Err(message) = &result {
        app.state::<UpdateState>()
            .installing
            .store(false, Ordering::SeqCst);
        publish_update_status(
            app,
            UpdateStatus::Error {
                message: message.clone(),
            },
        );
    }
    result
}

/// Download and install the pending update (checking first when there is
/// none), stop the managed `serve`, and relaunch into the new build.
async fn install_pending_update(app: &AppHandle) -> Result<(), String> {
    let pending = lock(&app.state::<UpdateState>().pending).clone();
    let update = match pending {
        Some(update) => update,
        None => match perform_update_check_inner(app).await {
            UpdateStatus::Available { .. } => lock(&app.state::<UpdateState>().pending)
                .clone()
                .ok_or("Update is no longer available")?,
            UpdateStatus::UpToDate { current } => {
                return Err(format!("Selara v{current} is already up to date"))
            }
            UpdateStatus::Unconfigured => return Err(
                "This local build has updates disabled. Install the notarized release from GitHub."
                    .into(),
            ),
            UpdateStatus::Error { message } => return Err(message),
            _ => return Err("An update is already in progress".into()),
        },
    };
    let version = update.version.clone();
    publish_update_status(
        app,
        UpdateStatus::Downloading {
            version: version.clone(),
            downloaded: 0,
            total: None,
        },
    );
    let mut downloaded = 0u64;
    let bytes = update
        .download(
            |count, total| {
                downloaded = downloaded.saturating_add(count as u64);
                publish_update_status(
                    app,
                    UpdateStatus::Downloading {
                        version: version.clone(),
                        downloaded,
                        total,
                    },
                );
            },
            || {},
        )
        .await
        .map_err(|e| format!("Could not verify the update download: {e}"))?;
    // download() verifies the signature AFTER its finish callback. Only this
    // successfully awaited result may pause workers or touch the installation.
    app_server::reset().await.map_err(|e| e.to_string())?;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let app_path = update_backup::running_app()?;
        let sup = handle.state::<Supervisor>();
        let _transition = lock(&sup.transition);
        let was_running = sup.managed_pid().is_some();
        if !was_running && serve_status().running {
            return Err(
                "Stop the externally started background service before installing this update"
                    .into(),
            );
        }
        // A blocked attempt must not leave a full app backup on every retry.
        // Keep this check and preparation in the same supervisor transition.
        let backup =
            update_backup::Backup::prepare(&app_path, &handle.package_info().version.to_string())?;
        let desired = sup.desired.swap(false, Ordering::SeqCst);
        publish_update_status(
            &handle,
            UpdateStatus::Waiting {
                version: version.clone(),
            },
        );
        if was_running {
            if let Err(error) = quiesce_serve_locked(&handle, &sup) {
                let resume = resume_serve_locked(&handle, &sup);
                sup.desired.store(desired, Ordering::SeqCst);
                return Err(format!(
                    "Could not pause active work: {error}{}",
                    resume
                        .err()
                        .map(|e| format!("; resume failed: {e}"))
                        .unwrap_or_default()
                ));
            }
            if let Err(error) = stop_serve_locked(&handle, &sup) {
                let resume = resume_serve_locked(&handle, &sup);
                sup.desired.store(desired, Ordering::SeqCst);
                return Err(format!(
                    "Could not stop the background process: {error}{}",
                    resume
                        .err()
                        .map(|e| format!("; resume failed: {e}"))
                        .unwrap_or_default()
                ));
            }
        }
        publish_update_status(
            &handle,
            UpdateStatus::Installing {
                version: version.clone(),
            },
        );
        match backup.install_archive(&version, &bytes) {
            Ok(()) => Ok(()),
            Err(failure) => {
                if failure.restored && was_running && desired {
                    if let Err(error) = start_serve_locked(&handle, &sup) {
                        return Err(format!(
                            "{}; background restart failed: {error}",
                            failure.message
                        ));
                    }
                }
                Err(failure.message)
            }
        }
    })
    .await
    .map_err(|e| format!("Update installation task failed: {e}"))??;
    // Tauri waits for ExitRequested/Exit when restarting off the main thread.
    // Those handlers stop the supervisor, so the blocking transaction must
    // return and release its transition and installation locks first. The
    // outer maintenance guard still prevents new work until this process exits.
    app.restart();
}

/// Startup + every 24 h: check in the background; `apply_update_status`
/// lights up the tray item when something is found.
fn spawn_update_checks(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(UPDATE_CHECK_STARTUP_DELAY);
        loop {
            tauri::async_runtime::block_on(perform_update_check(&app));
            std::thread::sleep(UPDATE_CHECK_INTERVAL);
        }
    });
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/// The last check's result, without touching the network. Lets the Settings
/// window recover a status that was emitted before its listener was bound.
#[tauri::command]
fn update_status(app: AppHandle) -> Option<UpdateStatus> {
    lock(&app.state::<UpdateState>().last).clone()
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<UpdateStatus, String> {
    Ok(perform_update_check(&app).await)
}

#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    perform_update_install(&app).await
}

fn show_settings<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Rewrites an existing login item with this build's executable and arguments.
/// The plugin only checks file existence, so initializing it alone would leave
/// legacy entries opening Settings at every login. Disabled stays disabled.
fn refresh_enabled_autostart<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    let autolaunch = app.autolaunch();
    let enabled = autolaunch.is_enabled().map_err(|e| e.to_string())?;
    if enabled {
        autolaunch.enable().map_err(|e| e.to_string())?;
    }
    Ok(enabled)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(Supervisor::default())
        .manage(UpdateState::default())
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            save_config_section,
            api_key_source,
            store_api_key,
            clear_api_key,
            export_commands,
            import_commands,
            config_path,
            history_list,
            history_clear,
            history_path,
            chatgpt_auth_status,
            chatgpt_login,
            chatgpt_login_cancel,
            chatgpt_logout,
            list_chatgpt_models_cmd,
            list_provider_models_cmd,
            serve_status,
            usage_summary,
            clear_usage,
            serve_start,
            serve_stop,
            serve_restart,
            serve_quiesce,
            serve_resume,
            serve_log,
            serve_supervisor_status,
            accessibility_status,
            open_accessibility_settings,
            app_version,
            check_for_updates,
            update_status,
            install_update
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let show_i = MenuItem::with_id(app, "show", "Open Settings", true, None::<&str>)?;
            let start_i = MenuItem::with_id(app, "serve-start", "Start serve", true, None::<&str>)?;
            let stop_i = MenuItem::with_id(app, "serve-stop", "Stop serve", false, None::<&str>)?;
            let restart_i =
                MenuItem::with_id(app, "serve-restart", "Restart serve", false, None::<&str>)?;
            let login_checked = refresh_enabled_autostart(app.handle()).unwrap_or_else(|e| {
                app.state::<Supervisor>().push_log(format!(
                    "[supervisor] could not refresh start at login: {e}"
                ));
                app.autolaunch().is_enabled().unwrap_or(false)
            });
            let login_i = CheckMenuItem::with_id(
                app,
                "login",
                "Start at login",
                true,
                login_checked,
                None::<&str>,
            )?;
            let check_update_i = MenuItem::with_id(
                app,
                "update-check",
                "Check for updates…",
                true,
                None::<&str>,
            )?;
            let update_i =
                MenuItem::with_id(app, "update", "No update checked yet", false, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show_i,
                    &PredefinedMenuItem::separator(app)?,
                    &start_i,
                    &stop_i,
                    &restart_i,
                    &PredefinedMenuItem::separator(app)?,
                    &login_i,
                    &PredefinedMenuItem::separator(app)?,
                    &check_update_i,
                    &update_i,
                    &PredefinedMenuItem::separator(app)?,
                    &quit_i,
                ],
            )?;
            app.manage(TrayMenu {
                start: start_i,
                stop: stop_i,
                restart: restart_i,
                login: login_i,
                update: update_i,
            });

            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Selara")
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => show_settings(app),
                    "serve-start" | "serve-stop" | "serve-restart" => {
                        // Off the main thread: stopping waits for the child.
                        let action: fn(&AppHandle) -> Result<(), String> = match event.id.as_ref() {
                            "serve-start" => start_serve,
                            "serve-stop" => stop_serve,
                            _ => restart_serve,
                        };
                        let app = app.clone();
                        std::thread::spawn(move || {
                            if let Err(e) = action(&app) {
                                app.state::<Supervisor>().set_error(Some(e));
                                notify(&app);
                            }
                        });
                    }
                    "login" => {
                        let al = app.autolaunch();
                        let result = if al.is_enabled().unwrap_or(false) {
                            al.disable()
                        } else {
                            al.enable()
                        };
                        if let Err(e) = result {
                            app.state::<Supervisor>()
                                .push_log(format!("[supervisor] start at login: {e}"));
                        }
                        notify(app);
                    }
                    "update-check" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            perform_update_check(&app).await;
                        });
                    }
                    "update" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = perform_update_install(&app).await {
                                app.state::<Supervisor>().push_log(format!("[updater] {e}"));
                                let status = UpdateStatus::Error { message: e };
                                *lock(&app.state::<UpdateState>().last) = Some(status.clone());
                                apply_update_status(&app, &status);
                            }
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_settings(tray.app_handle());
                    }
                });

            // Prefer bundled icon when present.
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            let _tray = tray.build(app)?;

            if let Some(win) = app.get_webview_window("settings") {
                #[cfg(target_os = "macos")]
                {
                    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
                    let _ = apply_vibrancy(&win, NSVisualEffectMaterial::Sidebar, None, None);
                }
                #[cfg(debug_assertions)]
                {
                    win.open_devtools();
                }
                let h = handle.clone();
                win.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Some(w) = h.get_webview_window("settings") {
                            let _ = w.hide();
                        }
                    }
                });
            }

            // Ensure config exists on first launch.
            let _ = get_config();
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Selara desktop")
        .run(|app, event| match event {
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => show_settings(app),
            // The event loop is up: start `serve` unless one already runs.
            RunEvent::Ready => {
                // Explicit app launches should expose Settings immediately;
                // login-item launches keep both Settings and the picker hidden.
                if !std::env::args_os().any(|arg| arg == "--background") {
                    show_settings(app);
                }
                let app = app.clone();
                spawn_update_checks(app.clone());
                std::thread::spawn(move || {
                    let _ = start_serve(&app);
                });
            }
            // Take the sidecar down with us so no orphan keeps the hotkey.
            RunEvent::ExitRequested { .. } | RunEvent::Exit => {
                let _ = stop_serve(app);
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::{updater_configured, UPDATER_PUBKEY_PLACEHOLDER};

    #[cfg(target_os = "macos")]
    #[test]
    fn legacy_login_item_migrates_without_enabling_disabled_login() {
        use std::{fs, process::Command};
        // The actual plugin writes a LaunchAgent. Isolate HOME in a child
        // process so this regression never changes the user's login items.
        if std::env::var_os("SELARA_AUTOSTART_TEST_CHILD").is_none() {
            let temporary = std::env::temp_dir().join(format!(
                "selara-autostart-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(temporary.join("Library/LaunchAgents")).unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::legacy_login_item_migrates_without_enabling_disabled_login",
                ])
                .env("SELARA_AUTOSTART_TEST_CHILD", "1")
                .env("HOME", &temporary)
                .status()
                .unwrap();
            fs::remove_dir_all(temporary).unwrap();
            assert!(status.success());
            return;
        }
        let plist = std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
            .join("Library/LaunchAgents/SelaraAutostartFixture.plist");
        fs::write(&plist, r#"<?xml version="1.0"?><plist version="1.0"><dict><key>ProgramArguments</key><array><string>/Applications/Selara.app/Contents/MacOS/selara-desktop</string></array></dict></plist>"#).unwrap();
        let app = tauri::test::mock_builder()
            .plugin(
                tauri_plugin_autostart::Builder::new()
                    .app_name("SelaraAutostartFixture")
                    .macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent)
                    .arg("--background")
                    .build(),
            )
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        assert!(super::refresh_enabled_autostart(app.handle()).unwrap());
        let migrated = fs::read_to_string(&plist).unwrap();
        assert!(migrated.contains("<string>--background</string>"));
        assert!(!migrated.contains("/Applications/Selara.app"));
        fs::remove_file(&plist).unwrap();
        assert!(!super::refresh_enabled_autostart(app.handle()).unwrap());
        assert!(!plist.exists());
    }

    #[test]
    fn placeholder_and_empty_pubkeys_fail_closed() {
        assert!(!updater_configured(UPDATER_PUBKEY_PLACEHOLDER));
        assert!(!updater_configured(&format!(
            "  {UPDATER_PUBKEY_PLACEHOLDER}\n"
        )));
        assert!(!updater_configured(""));
        assert!(!updater_configured("   "));
    }

    #[test]
    fn real_pubkey_is_configured() {
        // Shape of a `tauri signer generate` public key (base64 minisign).
        assert!(updater_configured(
            "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEFCQ0RFRgpSV1FBQkNERUY="
        ));
    }
}

#[cfg(test)]
mod update_transport_tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    use tauri_plugin_updater::UpdaterExt;

    #[tokio::test]
    async fn actual_updater_verifies_downloads_and_rejects_missing_tampered_or_interrupted_feeds() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/updater-signature.json")).unwrap();
        for scenario in ["valid", "missing", "tampered", "interrupted"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let payload = fixture["payload"].as_str().unwrap().as_bytes().to_vec();
            let manifest = serde_json::json!({"version":"0.2.0","platforms":{"darwin-aarch64":{"url":format!("{base}/app.tar.gz"),"signature":fixture["signature"]}}}).to_string();
            let expected = payload.clone();
            let server = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut remaining = if scenario == "missing" { 1 } else { 2 };
                while remaining > 0 && Instant::now() < deadline {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while !request.ends_with(b"\r\n\r\n") && request.len() < 16384 {
                        if stream.read(&mut byte).unwrap_or(0) == 0 {
                            break;
                        }
                        request.push(byte[0]);
                    }
                    let archive = String::from_utf8_lossy(&request).starts_with("GET /app.tar.gz ");
                    let (status, bytes) = if scenario == "missing" {
                        ("404 Not Found", b"missing".to_vec())
                    } else if archive && scenario == "tampered" {
                        ("200 OK", b"tampered".to_vec())
                    } else if archive {
                        ("200 OK", payload.clone())
                    } else {
                        ("200 OK", manifest.as_bytes().to_vec())
                    };
                    let size = bytes.len()
                        + if archive && scenario == "interrupted" {
                            100
                        } else {
                            0
                        };
                    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {size}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n");
                    let _ = stream.write_all(&bytes);
                    remaining -= 1;
                }
                assert_eq!(remaining, 0);
            });
            let mut context = tauri::test::mock_context(tauri::test::noop_assets());
            context.config_mut().plugins.0.insert("updater".into(), serde_json::json!({"dangerousInsecureTransportProtocol":true,"pubkey":fixture["publicKey"],"endpoints":[]}));
            let app = tauri::test::mock_builder()
                .plugin(
                    tauri_plugin_updater::Builder::new()
                        .pubkey(fixture["publicKey"].as_str().unwrap())
                        .build(),
                )
                .build(context)
                .unwrap();
            let updater = app
                .updater_builder()
                .target("darwin-aarch64")
                .endpoints(vec![format!("{base}/latest.json").parse().unwrap()])
                .unwrap()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap();
            let check = updater.check().await;
            if scenario == "missing" {
                assert!(check.is_err(), "missing feed must not be up to date");
            } else {
                let mut update = check.unwrap().unwrap();
                update.timeout = Some(Duration::from_secs(2));
                let result = update.download(|_, _| {}, || {}).await;
                if scenario == "valid" {
                    assert_eq!(result.unwrap(), expected);
                } else {
                    assert!(result.is_err(), "{scenario}");
                }
            }
            server.join().unwrap();
        }
    }
}
