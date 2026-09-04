use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::{Arc, Mutex};

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

/// Global hotkey registration. Construct and [`Self::register`] on the **main**
/// thread. Call [`Self::set_wake`] so a hotkey can revive a hidden egui window.
pub struct MacosHotkey {
    manager: Mutex<Option<GlobalHotKeyManager>>,
    registered: Mutex<Option<HotKey>>,
    on_fire: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Wakes the UI loop (egui `request_repaint`) so a hidden window still reacts.
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl MacosHotkey {
    pub fn new() -> Self {
        Self {
            manager: Mutex::new(None),
            registered: Mutex::new(None),
            on_fire: Mutex::new(None),
            wake: Mutex::new(None),
        }
    }

    /// Call before [`HotkeyService::register`]. Pass egui `ctx.request_repaint`
    /// so hotkeys revive a hidden / idle window.
    pub fn set_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self
            .wake
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(wake));
    }

    /// Drain the channel fallback. No-op when `set_event_handler` is active.
    pub fn poll(&self) {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            Self::dispatch_event(
                event,
                &self.registered,
                &self.on_fire,
                &self.wake,
            );
        }
    }

    fn dispatch_event(
        event: GlobalHotKeyEvent,
        registered: &Mutex<Option<HotKey>>,
        on_fire: &Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
        wake: &Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    ) {
        if event.state != HotKeyState::Pressed {
            return;
        }
        let id_ok = registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|h| h.id() == event.id)
            .unwrap_or(false);
        if !id_ok {
            return;
        }
        if let Some(cb) = on_fire
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            cb();
        }
        if let Some(w) = wake.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            w();
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
    async fn register(&self, hotkey: &str, on_fire: Box<dyn Fn() + Send + Sync>) -> Result<()> {
        let parsed = parse_hotkey(hotkey).with_context(|| format!("parse hotkey `{hotkey}`"))?;
        let manager =
            GlobalHotKeyManager::new().context("create GlobalHotKeyManager (main thread)")?;
        manager
            .register(parsed)
            .with_context(|| format!("register hotkey `{hotkey}`"))?;

        let hotkey_id = parsed.id();
        *self.on_fire.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::from(on_fire));
        *self.registered.lock().unwrap_or_else(|e| e.into_inner()) = Some(parsed);
        *self.manager.lock().unwrap_or_else(|e| e.into_inner()) = Some(manager);

        // Event handler wakes a hidden egui loop. OnceCell: first set wins.
        let on_fire = self
            .on_fire
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let wake = self
            .wake
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state != HotKeyState::Pressed || event.id != hotkey_id {
                return;
            }
            if let Some(cb) = on_fire.as_ref() {
                cb();
            }
            if let Some(w) = wake.as_ref() {
                w();
            }
        }));

        Ok(())
    }
}
