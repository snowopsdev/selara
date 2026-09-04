use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::HotkeyService;

/// Parse config strings like `ctrl+shift+space`, `option+space`, `cmd+shift+w`.
pub fn parse_hotkey(spec: &str) -> Result<HotKey> {
    let mut mods = Modifiers::empty();
    let mut key: Option<Code> = None;

    for part in spec.split('+').map(|s| s.trim().to_ascii_lowercase()) {
        if part.is_empty() {
            continue;
        }
        match part.as_str() {
            "ctrl" | "control" | "control_l" | "control_r" => mods |= Modifiers::CONTROL,
            "shift" => mods |= Modifiers::SHIFT,
            "alt" | "option" | "opt" => mods |= Modifiers::ALT,
            "cmd" | "command" | "super" | "meta" | "win" => mods |= Modifiers::META,
            "space" => key = Some(Code::Space),
            "enter" | "return" => key = Some(Code::Enter),
            "tab" => key = Some(Code::Tab),
            "escape" | "esc" => key = Some(Code::Escape),
            other if other.len() == 1 => {
                let c = other.chars().next().unwrap();
                key = Some(match c {
                    'a' => Code::KeyA,
                    'b' => Code::KeyB,
                    'c' => Code::KeyC,
                    'd' => Code::KeyD,
                    'e' => Code::KeyE,
                    'f' => Code::KeyF,
                    'g' => Code::KeyG,
                    'h' => Code::KeyH,
                    'i' => Code::KeyI,
                    'j' => Code::KeyJ,
                    'k' => Code::KeyK,
                    'l' => Code::KeyL,
                    'm' => Code::KeyM,
                    'n' => Code::KeyN,
                    'o' => Code::KeyO,
                    'p' => Code::KeyP,
                    'q' => Code::KeyQ,
                    'r' => Code::KeyR,
                    's' => Code::KeyS,
                    't' => Code::KeyT,
                    'u' => Code::KeyU,
                    'v' => Code::KeyV,
                    'w' => Code::KeyW,
                    'x' => Code::KeyX,
                    'y' => Code::KeyY,
                    'z' => Code::KeyZ,
                    '0' => Code::Digit0,
                    '1' => Code::Digit1,
                    '2' => Code::Digit2,
                    '3' => Code::Digit3,
                    '4' => Code::Digit4,
                    '5' => Code::Digit5,
                    '6' => Code::Digit6,
                    '7' => Code::Digit7,
                    '8' => Code::Digit8,
                    '9' => Code::Digit9,
                    _ => bail!("unsupported hotkey key: {other}"),
                });
            }
            other => bail!("unsupported hotkey token: {other}"),
        }
    }

    let Some(code) = key else {
        bail!("hotkey `{spec}` is missing a key (example: ctrl+shift+space)");
    };
    Ok(HotKey::new(Some(mods), code))
}

/// What a registered global hotkey should do when pressed.
#[derive(Debug, Clone)]
pub enum HotkeyAction {
    /// Open the command picker.
    Picker,
    /// Run this command id directly.
    Command(String),
}

struct SharedHotkeys {
    /// hotkey id → action
    by_id: Mutex<HashMap<u32, HotkeyAction>>,
    pending: Mutex<Option<HotkeyAction>>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl SharedHotkeys {
    fn handle(&self, event: GlobalHotKeyEvent) {
        if event.state != HotKeyState::Pressed {
            return;
        }
        let action = self
            .by_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&event.id)
            .cloned();
        let Some(action) = action else {
            return;
        };
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(action);
        if let Some(w) = self.wake.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            w();
        }
    }
}

static SHARED: OnceLock<Arc<SharedHotkeys>> = OnceLock::new();

fn shared() -> Arc<SharedHotkeys> {
    SHARED
        .get_or_init(|| {
            let s = Arc::new(SharedHotkeys {
                by_id: Mutex::new(HashMap::new()),
                pending: Mutex::new(None),
                wake: Mutex::new(None),
            });
            let s2 = s.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                s2.handle(event);
            }));
            s
        })
        .clone()
}

/// Global hotkey hub. Supports picker + many per-command bindings; call
/// [`MacosHotkey::reregister_all`] when config changes.
pub struct MacosHotkey {
    manager: Mutex<Option<GlobalHotKeyManager>>,
    shared: Arc<SharedHotkeys>,
}

impl MacosHotkey {
    pub fn new() -> Self {
        Self {
            manager: Mutex::new(None),
            shared: shared(),
        }
    }

    /// Call early so a hidden egui window still wakes on hotkey.
    pub fn set_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self
            .shared
            .wake
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(wake));
    }

    /// Take a pending action (picker or command id), if any.
    pub fn take_pending(&self) -> Option<HotkeyAction> {
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Unregister everything and register picker + command hotkeys.
    /// `command_hotkeys` is `(command_id, hotkey_spec)`.
    pub fn reregister_all(
        &self,
        picker: &str,
        command_hotkeys: &[(String, String)],
    ) -> Result<()> {
        let manager =
            GlobalHotKeyManager::new().context("create GlobalHotKeyManager (main thread)")?;

        let mut map = HashMap::new();

        let picker_hk =
            parse_hotkey(picker).with_context(|| format!("parse picker hotkey `{picker}`"))?;
        manager
            .register(picker_hk)
            .with_context(|| format!("register picker hotkey `{picker}`"))?;
        map.insert(picker_hk.id(), HotkeyAction::Picker);

        for (cmd_id, spec) in command_hotkeys {
            let spec = spec.trim();
            if spec.is_empty() {
                continue;
            }
            let hk = parse_hotkey(spec)
                .with_context(|| format!("parse command hotkey `{spec}` for `{cmd_id}`"))?;
            if map.contains_key(&hk.id()) {
                bail!("hotkey `{spec}` collides with another binding");
            }
            manager
                .register(hk)
                .with_context(|| format!("register command hotkey `{spec}` for `{cmd_id}`"))?;
            map.insert(hk.id(), HotkeyAction::Command(cmd_id.clone()));
        }

        *self
            .shared
            .by_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = map;
        *self.manager.lock().unwrap_or_else(|e| e.into_inner()) = Some(manager);
        Ok(())
    }

    /// Drain channel fallback (handler path is primary).
    pub fn poll(&self) {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            self.shared.handle(event);
        }
    }
}

impl Default for MacosHotkey {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HotkeyService for MacosHotkey {
    async fn register(&self, hotkey: &str, _on_fire: Box<dyn Fn() + Send + Sync>) -> Result<()> {
        // Legacy single-hotkey path: register as picker only.
        self.reregister_all(hotkey, &[])
    }
}
