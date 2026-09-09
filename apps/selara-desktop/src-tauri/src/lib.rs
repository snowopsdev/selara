use selara_core::codex_cli::{self, CodexLoginStatus};
use selara_core::commands::{
    merge_commands, parse_command_pack, render_command_pack, MergeMode, MergeReport,
};
use selara_core::config::{serve_pidfile, ApiKeySource, AppConfig};
use selara_core::providers::{list_chatgpt_models, list_provider_models, ProviderKind};
use selara_core::secrets;
use std::sync::{Mutex, MutexGuard};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, Runtime, WindowEvent,
};
use tauri_plugin_dialog::DialogExt;

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
        let report = merge_commands(&mut cfg.commands, incoming, mode);
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
            accessibility_status,
            open_accessibility_settings
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let show_i = MenuItem::with_id(app, "show", "Open Settings", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Selara")
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => show_settings(app),
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
        .run(tauri::generate_context!())
        .expect("error while running Selara desktop");
}
