//! Short-lived ownership of shortcut capture in the Settings window.
use super::*;

const LEASE: Duration = Duration::from_secs(3);
static OPERATION: Mutex<()> = Mutex::new(());
static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);

#[derive(Clone)]
struct Recording {
    session_id: String,
    deadline: Instant,
    service_generation: u64,
    managed_pid: Option<u32>,
}

pub(super) fn active() -> bool {
    lock(&RECORDING)
        .as_ref()
        .is_some_and(|recording| Instant::now() < recording.deadline)
}

// Never hold a supervisor/state lock while waiting for the native menu thread.
fn refresh_menu(app: &AppHandle) -> Result<(), String> {
    let handle = app.clone();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        refresh_command_menu_main(&handle);
        let _ = send.send(());
    })
    .map_err(|error| error.to_string())?;
    receive
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Settings did not finish preparing the shortcut recorder".into())
}

fn set_recording(app: &AppHandle, session_id: String, enabled: bool) -> Result<(), String> {
    let _operation = lock(&OPERATION);
    let sup = app.state::<Supervisor>();
    let previous = lock(&RECORDING).clone();
    if !enabled
        && previous
            .as_ref()
            .is_none_or(|state| state.session_id != session_id)
    {
        return Ok(()); // A late cleanup cannot release a newer recorder.
    }
    if enabled {
        let window = app
            .get_webview_window("settings")
            .ok_or("Settings is closed")?;
        if !window.is_focused().unwrap_or(false) {
            return Err("Keep Settings focused while recording a shortcut".into());
        }
        if MAINTENANCE.load(Ordering::SeqCst) {
            return Err("Finish the account change or update before recording a shortcut".into());
        }
        if previous
            .as_ref()
            .is_some_and(|state| state.session_id != session_id && Instant::now() < state.deadline)
        {
            return Err("Another shortcut is being recorded".into());
        }
    }
    if enabled {
        *lock(&RECORDING) = Some(Recording {
            session_id: session_id.clone(),
            deadline: Instant::now() + LEASE,
            service_generation: 0,
            managed_pid: None,
        });
        if previous
            .as_ref()
            .is_none_or(|state| state.session_id != session_id)
        {
            if let Err(error) = refresh_menu(app) {
                *lock(&RECORDING) = None;
                return Err(error);
            }
        }
    } else {
        *lock(&RECORDING) = None;
    }
    let (result, generation, managed_pid) = {
        let _transition = lock(&sup.transition);
        let generation = sup.generation.load(Ordering::SeqCst);
        let managed_pid = sup.managed_pid();
        let result = if enabled
            && previous.as_ref().is_some_and(|state| {
                state.session_id == session_id
                    && (state.service_generation != generation || state.managed_pid != managed_pid)
            }) {
            Err("The writing service changed. Click to record the shortcut again.".into())
        } else if managed_pid.is_some() {
            protocol_request_with_timeout_locked(
                app,
                &sup,
                selara_core::desktop_protocol::ProtocolCommand::SetShortcutRecording {
                    session_id: session_id.clone(),
                    active: enabled,
                    owner_pid: std::process::id() as i32,
                },
                Duration::from_secs(2),
            )
            .map(|_| ())
        } else if enabled && serve_status().running {
            Err("Stop the externally started service before recording shortcuts".into())
        } else {
            Ok(())
        };
        (result, generation, managed_pid)
    };
    if result.is_err() || !enabled {
        *lock(&RECORDING) = None;
        refresh_menu(app)?;
    } else if let Some(state) = lock(&RECORDING).as_mut() {
        state.deadline = Instant::now() + LEASE;
        state.service_generation = generation;
        state.managed_pid = managed_pid;
    }
    result
}

#[tauri::command]
pub(crate) async fn set_shortcut_recording(
    app: AppHandle,
    window: tauri::WebviewWindow,
    session_id: String,
    active: bool,
) -> Result<(), String> {
    if window.label() != "settings" || session_id.is_empty() || session_id.len() > 128 {
        return Err("Invalid shortcut recording request".into());
    }
    supervise(app, move |app| set_recording(app, session_id, active)).await
}

pub(super) fn release_on_blur(app: &AppHandle) {
    let session = lock(&RECORDING)
        .as_ref()
        .map(|state| state.session_id.clone());
    if let Some(session) = session {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = set_recording(&app, session, false) {
                app.state::<Supervisor>()
                    .push_log(format!("[shortcut recorder] {error}"));
            }
        });
    }
}
