use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, Runtime, WindowEvent,
};
use selara_core::codex_cli::{self, CodexLoginStatus};
use selara_core::config::AppConfig;
use selara_core::providers::list_chatgpt_models;

#[tauri::command]
fn get_config() -> Result<AppConfig, String> {
    let path = AppConfig::default_path();
    AppConfig::load_or_init(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_config(config: AppConfig) -> Result<(), String> {
    let path = AppConfig::default_path();
    config.save(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn config_path() -> String {
    AppConfig::default_path().display().to_string()
}

#[tauri::command]
fn chatgpt_auth_status() -> Result<CodexLoginStatus, String> {
    codex_cli::login_status().map_err(|e| e.to_string())
}

#[tauri::command]
fn chatgpt_login() -> Result<CodexLoginStatus, String> {
    // Spawns the Codex CLI browser login flow; may take a while.
    codex_cli::login().map_err(|e| e.to_string())
}

#[tauri::command]
fn chatgpt_logout() -> Result<CodexLoginStatus, String> {
    codex_cli::logout().map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_chatgpt_models_cmd() -> Result<Vec<String>, String> {
    list_chatgpt_models().await.map_err(|e| e.to_string())
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
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            config_path,
            chatgpt_auth_status,
            chatgpt_login,
            chatgpt_logout,
            list_chatgpt_models_cmd
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
