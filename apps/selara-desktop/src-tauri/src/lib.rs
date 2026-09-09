use selara_core::codex_cli::{self, CodexLoginStatus};
use selara_core::commands::{
    merge_commands, parse_command_pack, render_command_pack, MergeMode, MergeReport,
};
use selara_core::config::{serve_pidfile, ApiKeySource, AppConfig};
use selara_core::providers::{list_chatgpt_models, list_provider_models, ProviderKind};
use selara_core::secrets;
use std::collections::VecDeque;
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

/// Run a blocking `codex_cli` call off the main thread. Tauri 2 executes sync
/// commands on the main thread, which would freeze the Settings window for the
/// duration of a `Command::status()` wait (the browser login flow in particular).
async fn run_codex_blocking<F>(f: F) -> Result<CodexLoginStatus, String>
where
    F: FnOnce() -> Result<CodexLoginStatus, selara_core::error::CoreError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("codex task failed: {e}"))?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn chatgpt_auth_status() -> Result<CodexLoginStatus, String> {
    run_codex_blocking(codex_cli::login_status).await
}

#[tauri::command]
async fn chatgpt_login() -> Result<CodexLoginStatus, String> {
    // Spawns the Codex CLI browser login flow; blocks until the user finishes
    // signing in, so it must not run on the main thread.
    run_codex_blocking(codex_cli::login).await
}

#[tauri::command]
async fn chatgpt_logout() -> Result<CodexLoginStatus, String> {
    run_codex_blocking(codex_cli::logout).await
}

#[tauri::command]
async fn list_chatgpt_models_cmd() -> Result<Vec<String>, String> {
    list_chatgpt_models().await.map_err(|e| e.to_string())
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

/// Whether this process is trusted for macOS Accessibility. `serve` needs the
/// same grant, but for the binary that runs it (Terminal, iTerm, or the app).
#[tauri::command]
fn accessibility_status() -> bool {
    #[cfg(target_os = "macos")]
    {
        selara_platform::macos::accessibility_trusted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Open System Settings on the Privacy & Security -> Accessibility pane.
#[tauri::command]
#[allow(deprecated)] // shell.open still ships with the shell plugin we already bundle
fn open_accessibility_settings(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_shell::ShellExt;
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
    if let Some(menu) = app.try_state::<TrayMenu>() {
        let managed = app.state::<Supervisor>().managed_pid().is_some();
        let external = !managed && serve_status().running;
        let _ = menu.start.set_enabled(!managed && !external);
        let _ = menu.stop.set_enabled(managed);
        let _ = menu.restart.set_enabled(managed);
        let _ = menu
            .login
            .set_checked(app.autolaunch().is_enabled().unwrap_or(false));
    }
    let _ = app.emit("serve-changed", ());
}

/// Spawn the `serve` sidecar unless one (ours or external) already runs.
fn start_serve(app: &AppHandle) -> Result<(), String> {
    let sup = app.state::<Supervisor>();
    let _transition = lock(&sup.transition);
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

    let spawned = app
        .shell()
        .sidecar("selara")
        .and_then(|cmd| cmd.args(["serve"]).spawn());
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
    let generation = sup.generation.fetch_add(1, Ordering::SeqCst) + 1;
    sup.desired.store(true, Ordering::SeqCst);
    sup.set_error(None);
    *lock(&sup.child) = Some(child);
    sup.push_log(format!("started serve (pid {pid})"));
    notify(app);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            let sup = app.state::<Supervisor>();
            match event {
                CommandEvent::Stdout(bytes) | CommandEvent::Stderr(bytes) => {
                    let line = String::from_utf8_lossy(&bytes);
                    let line = line.trim_end();
                    if !line.is_empty() {
                        sup.push_log(line.to_string());
                    }
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

/// Called from the reader task when our child exits. Restarts with backoff
/// while `desired`, unless it keeps dying (`MAX_RESTARTS` in `RESTART_WINDOW`).
fn on_terminated(app: &AppHandle, generation: u64, payload: TerminatedPayload) {
    let sup = app.state::<Supervisor>();
    if sup.generation.load(Ordering::SeqCst) != generation {
        // stop_serve or a newer start already took over this slot.
        return;
    }
    *lock(&sup.child) = None;
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
    sup.generation.fetch_add(1, Ordering::SeqCst);
    let Some(child) = lock(&sup.child).take() else {
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
    let mut result = Ok(());
    if pid_alive(pid) {
        result = child
            .kill()
            .map_err(|e| format!("could not kill serve (pid {pid}): {e}"));
        let deadline = Instant::now() + Duration::from_secs(1);
        while pid_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    sup.push_log(format!("stopped serve (pid {pid})"));
    notify(app);
    result
}

fn restart_serve(app: &AppHandle) -> Result<(), String> {
    let sup = app.state::<Supervisor>();
    // One transition, so nothing can start a second sidecar in the window
    // between the stop and the start.
    let _transition = lock(&sup.transition);
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
}

#[tauri::command]
fn serve_supervisor_status(app: AppHandle) -> SupervisorStatus {
    let sup = app.state::<Supervisor>();
    let managed = sup.managed_pid();
    let last_error = lock(&sup.last_error).clone();
    let status = serve_status();
    SupervisorStatus {
        managed: managed.is_some(),
        pid: managed.or(status.running.then_some(status.pid).flatten()),
        external: managed.is_none() && status.running,
        last_error,
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
            UpdateStatus::Error { .. } => ("Update check failed".to_string(), false),
        };
        // An install in flight owns this item: a check that finishes in the
        // middle of one must not re-enable it.
        let installing = app
            .try_state::<UpdateState>()
            .is_some_and(|s| s.installing.load(Ordering::SeqCst));
        let _ = menu.update.set_text(if installing {
            "Installing update…".to_string()
        } else {
            text
        });
        let _ = menu.update.set_enabled(enabled && !installing);
    }
    let _ = app.emit("update-changed", status.clone());
}

/// Ask the release feed for a newer build. Never contacts the network while
/// the placeholder public key is configured.
async fn perform_update_check(app: &AppHandle) -> UpdateStatus {
    let current = app.package_info().version.to_string();
    let state = app.state::<UpdateState>();
    let status = if !updater_configured(&configured_pubkey(app)) {
        UpdateStatus::Unconfigured
    } else {
        let result = async {
            let updater = app.updater().map_err(|e| e.to_string())?;
            updater.check().await.map_err(|e| e.to_string())
        }
        .await;
        match result {
            Ok(Some(update)) => {
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
    if result.is_err() {
        // Only a failure releases the guard; a success restarts the process.
        app.state::<UpdateState>()
            .installing
            .store(false, Ordering::SeqCst);
        let last = lock(&app.state::<UpdateState>().last).clone();
        if let Some(status) = last {
            apply_update_status(app, &status);
        }
    }
    result
}

/// Download and install the pending update (checking first when there is
/// none), stop the managed `serve`, and relaunch into the new build.
async fn install_pending_update(app: &AppHandle) -> Result<(), String> {
    let pending = lock(&app.state::<UpdateState>().pending).clone();
    let update = match pending {
        Some(update) => update,
        None => match perform_update_check(app).await {
            UpdateStatus::Available { .. } => lock(&app.state::<UpdateState>().pending)
                .clone()
                .ok_or_else(|| "update vanished between check and install".to_string())?,
            UpdateStatus::UpToDate { current } => {
                return Err(format!("Selara v{current} is already the latest version"))
            }
            UpdateStatus::Unconfigured => {
                return Err(
                    "this build has no updater signing key; download releases from GitHub"
                        .to_string(),
                )
            }
            UpdateStatus::Error { message } => return Err(message),
        },
    };
    app.state::<Supervisor>()
        .push_log(format!("[updater] installing v{}", update.version));
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("could not install update: {e}"))?;
    // Take the sidecar down first so the relaunched app can start its own.
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = stop_serve(&handle);
        handle.restart();
    })
    .await
    .map_err(|e| format!("restart task failed: {e}"))
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
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
            chatgpt_auth_status,
            chatgpt_login,
            chatgpt_logout,
            list_chatgpt_models_cmd,
            list_provider_models_cmd,
            serve_status,
            serve_start,
            serve_stop,
            serve_restart,
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
            let login_checked = app.autolaunch().is_enabled().unwrap_or(false);
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
            // The event loop is up: start `serve` unless one already runs.
            RunEvent::Ready => {
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
