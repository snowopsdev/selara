use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tracing::warn;

use crate::HotkeyService;

/// Parse config strings like `ctrl+shift+space`, `option+space`, `cmd+shift+w`.
///
/// Tokens are separated by `+` and matched case-insensitively after trimming
/// whitespace, so `" Ctrl + Shift + Space "` parses like `ctrl+shift+space`.
/// Modifiers accumulate; if a spec names more than one key, the last one wins.
pub fn parse_hotkey(spec: &str) -> Result<HotKey> {
    let mut mods = Modifiers::empty();
    let mut key: Option<Code> = None;

    for part in spec.split('+').map(|s| s.trim().to_ascii_lowercase()) {
        if part.is_empty() {
            continue;
        }
        if let Some(m) = modifier_token(&part) {
            mods |= m;
        } else if let Some(code) = key_token(&part) {
            key = Some(code);
        } else if part.len() == 1 {
            bail!("unsupported hotkey key: {part}");
        } else {
            bail!("unsupported hotkey token: {part}");
        }
    }

    let Some(code) = key else {
        bail!("hotkey `{spec}` is missing a key (example: ctrl+shift+space)");
    };
    Ok(HotKey::new(Some(mods), code))
}

/// Map a lowercased modifier alias to its flag.
fn modifier_token(token: &str) -> Option<Modifiers> {
    Some(match token {
        "ctrl" | "control" | "control_l" | "control_r" => Modifiers::CONTROL,
        "shift" => Modifiers::SHIFT,
        "alt" | "option" | "opt" => Modifiers::ALT,
        "cmd" | "command" | "super" | "meta" | "win" => Modifiers::META,
        _ => return None,
    })
}

/// Map a lowercased key token (named key, letter, digit, F-key, arrow,
/// editing key, or punctuation character) to its key code.
fn key_token(token: &str) -> Option<Code> {
    Some(match token {
        "space" => Code::Space,
        "enter" | "return" => Code::Enter,
        "tab" => Code::Tab,
        "escape" | "esc" => Code::Escape,
        "backspace" => Code::Backspace,
        "delete" | "del" => Code::Delete,
        "home" => Code::Home,
        "end" => Code::End,
        "pageup" | "pgup" => Code::PageUp,
        "pagedown" | "pgdn" => Code::PageDown,
        "up" | "arrowup" => Code::ArrowUp,
        "down" | "arrowdown" => Code::ArrowDown,
        "left" | "arrowleft" => Code::ArrowLeft,
        "right" | "arrowright" => Code::ArrowRight,
        "f1" => Code::F1,
        "f2" => Code::F2,
        "f3" => Code::F3,
        "f4" => Code::F4,
        "f5" => Code::F5,
        "f6" => Code::F6,
        "f7" => Code::F7,
        "f8" => Code::F8,
        "f9" => Code::F9,
        "f10" => Code::F10,
        "f11" => Code::F11,
        "f12" => Code::F12,
        "a" => Code::KeyA,
        "b" => Code::KeyB,
        "c" => Code::KeyC,
        "d" => Code::KeyD,
        "e" => Code::KeyE,
        "f" => Code::KeyF,
        "g" => Code::KeyG,
        "h" => Code::KeyH,
        "i" => Code::KeyI,
        "j" => Code::KeyJ,
        "k" => Code::KeyK,
        "l" => Code::KeyL,
        "m" => Code::KeyM,
        "n" => Code::KeyN,
        "o" => Code::KeyO,
        "p" => Code::KeyP,
        "q" => Code::KeyQ,
        "r" => Code::KeyR,
        "s" => Code::KeyS,
        "t" => Code::KeyT,
        "u" => Code::KeyU,
        "v" => Code::KeyV,
        "w" => Code::KeyW,
        "x" => Code::KeyX,
        "y" => Code::KeyY,
        "z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "-" => Code::Minus,
        "=" => Code::Equal,
        "[" => Code::BracketLeft,
        "]" => Code::BracketRight,
        ";" => Code::Semicolon,
        "'" => Code::Quote,
        "," => Code::Comma,
        "." => Code::Period,
        "/" => Code::Slash,
        "`" => Code::Backquote,
        "\\" => Code::Backslash,
        _ => return None,
    })
}

/// What a registered global hotkey should do when pressed.
#[derive(Debug, Clone)]
pub enum HotkeyAction {
    /// Open the custom-instruction dialog.
    CustomInstruction,
    /// Run this command id directly.
    Command(String),
    /// Cancel the active command when Escape cancellation is enabled.
    Cancel,
}

/// A native registration that was valid and unique but could not be claimed
/// by macOS yet. These bindings are retried as a group without rebuilding the
/// manager or disturbing bindings that already registered successfully.
#[derive(Debug, Clone)]
struct PendingBinding {
    hotkey: HotKey,
    action: HotkeyAction,
    spec: String,
    label: String,
}

struct RegistrationTransition<'a> {
    epoch: &'a AtomicU64,
    active: &'a AtomicU64,
}

impl Drop for RegistrationTransition<'_> {
    fn drop(&mut self) {
        // Publish a new epoch before marking the transition inactive. A
        // handler that observes the inactive state therefore also observes
        // the completed registration writes.
        self.epoch.fetch_add(1, Ordering::Release);
        self.active.fetch_sub(1, Ordering::Release);
    }
}

struct SharedHotkeys {
    /// Serialize event dispatch with registration changes. An event already
    /// delivered by macOS must not become pending after `suspend` clears the
    /// queue.
    registration_lock: Mutex<()>,
    /// Event handlers capture the epoch before waiting on `registration_lock`
    /// and discard events that straddle a transition. The active count also
    /// handles multiple callers beginning transitions before one gets the
    /// registration lock.
    registration_epoch: AtomicU64,
    registration_active: AtomicU64,
    /// hotkey id → action
    by_id: Mutex<HashMap<u32, HotkeyAction>>,
    /// Valid, unique bindings whose native registration failed transiently.
    pending_bindings: Mutex<Vec<PendingBinding>>,
    pending: Mutex<Option<HotkeyAction>>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Whether the active run wants Escape cancellation. This is kept
    /// separate from `cancel_enabled`, which only becomes true after the
    /// native Escape registration succeeds.
    cancel_requested: AtomicBool,
    cancel_enabled: AtomicBool,
}

impl SharedHotkeys {
    fn begin_registration_transition(&self) -> RegistrationTransition<'_> {
        self.registration_active.fetch_add(1, Ordering::AcqRel);
        self.registration_epoch.fetch_add(1, Ordering::AcqRel);
        RegistrationTransition {
            epoch: &self.registration_epoch,
            active: &self.registration_active,
        }
    }

    fn handle(&self, event: GlobalHotKeyEvent) {
        let captured_epoch = self.registration_epoch.load(Ordering::Acquire);
        if self.registration_active.load(Ordering::Acquire) != 0 {
            return;
        }
        self.handle_at_epoch(event, captured_epoch);
    }

    fn handle_at_epoch(&self, event: GlobalHotKeyEvent, captured_epoch: u64) {
        if self.registration_active.load(Ordering::Acquire) != 0 {
            return;
        }
        let _registration = self
            .registration_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.registration_active.load(Ordering::Acquire) != 0
            || self.registration_epoch.load(Ordering::Acquire) != captured_epoch
        {
            return;
        }
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
        if matches!(action, HotkeyAction::Cancel) && !self.cancel_enabled.load(Ordering::Acquire) {
            return;
        }
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
                registration_lock: Mutex::new(()),
                registration_epoch: AtomicU64::new(0),
                registration_active: AtomicU64::new(0),
                by_id: Mutex::new(HashMap::new()),
                pending_bindings: Mutex::new(Vec::new()),
                pending: Mutex::new(None),
                wake: Mutex::new(None),
                cancel_requested: AtomicBool::new(false),
                cancel_enabled: AtomicBool::new(false),
            });
            let s2 = s.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                s2.handle(event);
            }));
            s
        })
        .clone()
}

/// Global hotkey hub. Supports custom-instruction + many per-command bindings; call
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
        *self.shared.wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(wake));
    }

    /// Take a pending custom-instruction, command, or cancellation action.
    pub fn take_pending(&self) -> Option<HotkeyAction> {
        let _registration = self
            .shared
            .registration_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Release every native registration while another application records a
    /// shortcut. The shared dispatch lock makes the clear atomic with respect
    /// to an in-flight event handler, so a key event cannot run after this
    /// method returns.
    pub fn suspend(&self) {
        let _transition = self.shared.begin_registration_transition();
        let _registration = self
            .shared
            .registration_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Clear the dispatch state before tearing down the native manager. A
        // platform event arriving during manager teardown then sees no action.
        self.shared.cancel_requested.store(false, Ordering::Release);
        self.shared.cancel_enabled.store(false, Ordering::Release);
        self.shared
            .by_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.shared
            .pending_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let manager = self
            .manager
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        drop(manager);
    }

    /// Enable or disable the Escape binding used to cancel the active run.
    /// The binding is registered only while enabled, so idle applications keep
    /// their native Escape behavior.
    pub fn set_cancel_enabled(&self, enabled: bool) -> Result<()> {
        let _transition = self.shared.begin_registration_transition();
        let _registration = self
            .shared
            .registration_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let cancel_hk = HotKey::new(None, Code::Escape);
        self.shared
            .cancel_requested
            .store(enabled, Ordering::Release);
        let manager = self.manager.lock().unwrap_or_else(|e| e.into_inner());
        let Some(manager) = manager.as_ref() else {
            if !enabled {
                self.shared.cancel_enabled.store(false, Ordering::Release);
                self.shared
                    .by_id
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&cancel_hk.id());
                self.shared
                    .pending_bindings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|binding| !matches!(&binding.action, HotkeyAction::Cancel));
                let mut pending = self
                    .shared
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if matches!(pending.as_ref(), Some(HotkeyAction::Cancel)) {
                    pending.take();
                }
                return Ok(());
            }
            return Err(anyhow::anyhow!(
                "hotkeys have not been registered; cannot change Escape cancellation"
            ));
        };
        if enabled {
            if self
                .shared
                .by_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&cancel_hk.id())
            {
                self.shared.cancel_enabled.store(true, Ordering::Release);
                return Ok(());
            }
            if self
                .shared
                .pending_bindings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|binding| matches!(&binding.action, HotkeyAction::Cancel))
            {
                return Err(anyhow::anyhow!(
                    "Escape cancellation hotkey registration is pending"
                ));
            }
            manager
                .register(cancel_hk)
                .context("register Escape cancellation hotkey")?;
            self.shared
                .by_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(cancel_hk.id(), HotkeyAction::Cancel);
            self.shared.cancel_enabled.store(true, Ordering::Release);
        } else {
            self.shared.cancel_enabled.store(false, Ordering::Release);
            self.shared
                .pending_bindings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|binding| !matches!(&binding.action, HotkeyAction::Cancel));
            let mut pending = self
                .shared
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if matches!(pending.as_ref(), Some(HotkeyAction::Cancel)) {
                pending.take();
            }
            let was_registered = self
                .shared
                .by_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&cancel_hk.id());
            if was_registered {
                manager
                    .unregister(cancel_hk)
                    .context("unregister Escape cancellation hotkey")?;
                self.shared
                    .by_id
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&cancel_hk.id());
            }
        }
        Ok(())
    }

    /// Unregister everything and register the custom-instruction and command
    /// hotkeys. `command_hotkeys` is `(command_id, hotkey_spec)`.
    ///
    /// Invalid, reserved, and duplicate individual bindings are reported and
    /// skipped. A valid binding whose native registration fails is retained
    /// for [`Self::retry_pending`] and counted in the returned value, so one
    /// unavailable shortcut cannot disable the others.
    ///
    /// The return value is the number of transient individual registrations
    /// still pending. A value of zero means registration is complete.
    pub fn reregister_all(
        &self,
        custom_instruction: &str,
        command_hotkeys: &[(String, String)],
    ) -> Result<usize> {
        let _transition = self.shared.begin_registration_transition();
        let _registration = self
            .shared
            .registration_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let pending_bindings = self
            .shared
            .pending_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let cancel_was_requested = cancellation_is_requested(
            self.shared.cancel_requested.load(Ordering::Acquire),
            &pending_bindings,
        );
        drop(pending_bindings);
        self.shared.cancel_enabled.store(false, Ordering::Release);
        self.shared
            .by_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.shared
            .pending_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        // Drop the previous manager before creating the replacement. macOS
        // rejects duplicate registrations while the old manager is alive.
        let old_manager = self
            .manager
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        drop(old_manager);
        let manager =
            GlobalHotKeyManager::new().context("create GlobalHotKeyManager (main thread)")?;

        let mut map = HashMap::new();
        let mut claimed = HashSet::new();
        let mut pending = Vec::new();
        let mut register = |hk: HotKey| manager.register(hk).map_err(|e| e.to_string());

        register_binding(
            &mut map,
            &mut claimed,
            &mut pending,
            custom_instruction,
            HotkeyAction::CustomInstruction,
            "custom instruction",
            &mut register,
        );

        for (cmd_id, spec) in command_hotkeys {
            register_binding(
                &mut map,
                &mut claimed,
                &mut pending,
                spec,
                HotkeyAction::Command(cmd_id.clone()),
                &format!("command `{cmd_id}`"),
                &mut register,
            );
        }

        if cancel_was_requested {
            let cancel_hk = HotKey::new(None, Code::Escape);
            register_cancel_binding(
                &mut map,
                &mut pending,
                cancel_hk,
                &mut register,
                &self.shared.cancel_enabled,
            );
        }

        *self.shared.by_id.lock().unwrap_or_else(|e| e.into_inner()) = map;
        *self
            .shared
            .pending_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = pending;
        *self.manager.lock().unwrap_or_else(|e| e.into_inner()) = Some(manager);
        Ok(self
            .shared
            .pending_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len())
    }

    /// Retry valid bindings whose previous native registration failed. The
    /// existing manager and successful dispatch map are kept in place, so a
    /// retry cannot unregister shortcuts that already work.
    ///
    /// Returns the number of transient registrations still pending. Invalid,
    /// reserved, and duplicate configuration entries never enter this queue.
    pub fn retry_pending(&self) -> Result<usize> {
        let _transition = self.shared.begin_registration_transition();
        let _registration = self
            .shared
            .registration_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let mut map = self.shared.by_id.lock().unwrap_or_else(|e| e.into_inner());
        let mut pending = self
            .shared
            .pending_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if pending.is_empty() {
            return Ok(0);
        }

        let manager = self.manager.lock().unwrap_or_else(|e| e.into_inner());
        let Some(manager) = manager.as_ref() else {
            return Err(anyhow::anyhow!(
                "hotkeys have not been registered; cannot retry pending bindings"
            ));
        };
        let mut register = |hk: HotKey| manager.register(hk).map_err(|e| e.to_string());
        let (remaining, escape_restored) =
            retry_pending_bindings(&mut map, &mut pending, &mut register);
        if escape_restored {
            self.shared.cancel_enabled.store(true, Ordering::Release);
        }
        Ok(remaining)
    }

    /// Drain channel fallback (handler path is primary).
    pub fn poll(&self) {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            self.shared.handle(event);
        }
    }
}

fn is_reserved_native(hk: &HotKey) -> bool {
    let cmd = Modifiers::SUPER;
    match (hk.mods, hk.key) {
        (mods, Code::KeyZ) => mods == cmd || mods == (cmd | Modifiers::SHIFT),
        (mods, Code::KeyC | Code::KeyX | Code::KeyV) => mods == cmd,
        _ => false,
    }
}

fn cancellation_is_requested(cancel_requested: bool, pending: &[PendingBinding]) -> bool {
    cancel_requested
        || pending
            .iter()
            .any(|binding| matches!(&binding.action, HotkeyAction::Cancel))
}

fn register_binding<F>(
    map: &mut HashMap<u32, HotkeyAction>,
    claimed: &mut HashSet<u32>,
    pending: &mut Vec<PendingBinding>,
    spec: &str,
    action: HotkeyAction,
    label: &str,
    register: &mut F,
) where
    F: FnMut(HotKey) -> std::result::Result<(), String>,
{
    let spec = spec.trim();
    if spec.is_empty() {
        return;
    }
    let hk = match parse_hotkey(spec) {
        Ok(hk) => hk,
        Err(e) => {
            warn!(binding = %spec, target = %label, error = %e, "ignoring invalid hotkey");
            return;
        }
    };
    if is_reserved_native(&hk) || hk == HotKey::new(None, Code::Escape) {
        warn!(binding = %spec, target = %label, "ignoring native editing hotkey");
        return;
    }
    if map.contains_key(&hk.id()) || !claimed.insert(hk.id()) {
        warn!(binding = %spec, target = %label, "ignoring duplicate hotkey");
        return;
    }
    if let Err(error) = register(hk) {
        pending.push(PendingBinding {
            hotkey: hk,
            action,
            spec: spec.to_string(),
            label: label.to_string(),
        });
        warn!(binding = %spec, target = %label, error = %error, "could not register hotkey; will retry");
        return;
    }
    map.insert(hk.id(), action);
}

fn register_cancel_binding<F>(
    map: &mut HashMap<u32, HotkeyAction>,
    pending: &mut Vec<PendingBinding>,
    hotkey: HotKey,
    register: &mut F,
    cancel_enabled: &AtomicBool,
) where
    F: FnMut(HotKey) -> std::result::Result<(), String>,
{
    if let Err(error) = register(hotkey) {
        pending.push(PendingBinding {
            hotkey,
            action: HotkeyAction::Cancel,
            spec: hotkey.to_string(),
            label: "Escape cancellation".to_string(),
        });
        warn!(error = %error, "could not restore Escape cancellation hotkey; will retry");
        return;
    }
    map.insert(hotkey.id(), HotkeyAction::Cancel);
    cancel_enabled.store(true, Ordering::Release);
}

fn retry_pending_bindings<F>(
    map: &mut HashMap<u32, HotkeyAction>,
    pending: &mut Vec<PendingBinding>,
    register: &mut F,
) -> (usize, bool)
where
    F: FnMut(HotKey) -> std::result::Result<(), String>,
{
    let mut remaining = Vec::with_capacity(pending.len());
    let mut escape_restored = false;
    for binding in pending.drain(..) {
        let id = binding.hotkey.id();
        match register(binding.hotkey) {
            Ok(()) => {
                escape_restored |= matches!(&binding.action, HotkeyAction::Cancel);
                map.insert(id, binding.action);
            }
            Err(error) => {
                tracing::debug!(
                    binding = %binding.spec,
                    target = %binding.label,
                    error = %error,
                    "could not register pending hotkey; will retry"
                );
                remaining.push(binding);
            }
        }
    }
    let count = remaining.len();
    *pending = remaining;
    (count, escape_restored)
}

impl Default for MacosHotkey {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HotkeyService for MacosHotkey {
    async fn register(&self, hotkey: &str, _on_fire: Box<dyn Fn() + Send + Sync>) -> Result<()> {
        // Legacy single-hotkey path: register as custom instruction.
        self.reregister_all(hotkey, &[]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(mods: Modifiers, code: Code) -> HotKey {
        HotKey::new(Some(mods), code)
    }

    fn parse(spec: &str) -> HotKey {
        parse_hotkey(spec).unwrap_or_else(|e| panic!("`{spec}` should parse: {e}"))
    }

    fn err(spec: &str) -> String {
        match parse_hotkey(spec) {
            Ok(hk) => panic!("`{spec}` should fail, parsed as {hk:?}"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn modifier_aliases_map_to_flags() {
        let cases = [
            ("ctrl", Modifiers::CONTROL),
            ("control", Modifiers::CONTROL),
            ("control_l", Modifiers::CONTROL),
            ("control_r", Modifiers::CONTROL),
            ("shift", Modifiers::SHIFT),
            ("alt", Modifiers::ALT),
            ("option", Modifiers::ALT),
            ("opt", Modifiers::ALT),
            ("cmd", Modifiers::META),
            ("command", Modifiers::META),
            ("super", Modifiers::META),
            ("meta", Modifiers::META),
            ("win", Modifiers::META),
        ];
        for (alias, flag) in cases {
            assert_eq!(
                parse(&format!("{alias}+a")),
                hk(flag, Code::KeyA),
                "{alias}"
            );
        }
    }

    #[test]
    fn modifiers_accumulate() {
        assert_eq!(
            parse("ctrl+shift+alt+cmd+space"),
            hk(
                Modifiers::CONTROL | Modifiers::SHIFT | Modifiers::ALT | Modifiers::META,
                Code::Space
            )
        );
    }

    #[test]
    fn named_keys() {
        let cases = [
            ("space", Code::Space),
            ("enter", Code::Enter),
            ("return", Code::Enter),
            ("tab", Code::Tab),
            ("escape", Code::Escape),
            ("esc", Code::Escape),
        ];
        for (token, code) in cases {
            assert_eq!(parse(token), hk(Modifiers::empty(), code), "{token}");
        }
    }

    #[test]
    fn every_letter() {
        let codes = [
            Code::KeyA,
            Code::KeyB,
            Code::KeyC,
            Code::KeyD,
            Code::KeyE,
            Code::KeyF,
            Code::KeyG,
            Code::KeyH,
            Code::KeyI,
            Code::KeyJ,
            Code::KeyK,
            Code::KeyL,
            Code::KeyM,
            Code::KeyN,
            Code::KeyO,
            Code::KeyP,
            Code::KeyQ,
            Code::KeyR,
            Code::KeyS,
            Code::KeyT,
            Code::KeyU,
            Code::KeyV,
            Code::KeyW,
            Code::KeyX,
            Code::KeyY,
            Code::KeyZ,
        ];
        assert_eq!(codes.len(), 26);
        for (letter, code) in ('a'..='z').zip(codes) {
            assert_eq!(
                parse(&letter.to_string()),
                hk(Modifiers::empty(), code),
                "{letter}"
            );
            let upper = letter.to_ascii_uppercase().to_string();
            assert_eq!(parse(&upper), hk(Modifiers::empty(), code), "{upper}");
        }
    }

    #[test]
    fn every_digit() {
        let codes = [
            Code::Digit0,
            Code::Digit1,
            Code::Digit2,
            Code::Digit3,
            Code::Digit4,
            Code::Digit5,
            Code::Digit6,
            Code::Digit7,
            Code::Digit8,
            Code::Digit9,
        ];
        assert_eq!(codes.len(), 10);
        for (digit, code) in ('0'..='9').zip(codes) {
            assert_eq!(
                parse(&digit.to_string()),
                hk(Modifiers::empty(), code),
                "{digit}"
            );
        }
    }

    #[test]
    fn function_keys() {
        let codes = [
            Code::F1,
            Code::F2,
            Code::F3,
            Code::F4,
            Code::F5,
            Code::F6,
            Code::F7,
            Code::F8,
            Code::F9,
            Code::F10,
            Code::F11,
            Code::F12,
        ];
        assert_eq!(codes.len(), 12);
        for (n, code) in (1..=12).zip(codes) {
            assert_eq!(
                parse(&format!("f{n}")),
                hk(Modifiers::empty(), code),
                "f{n}"
            );
            assert_eq!(
                parse(&format!("cmd+F{n}")),
                hk(Modifiers::META, code),
                "F{n}"
            );
        }
        assert!(parse_hotkey("f13").is_err(), "only F1-F12 are accepted");
        assert!(parse_hotkey("f0").is_err());
    }

    #[test]
    fn arrow_keys() {
        let cases = [
            ("up", Code::ArrowUp),
            ("arrowup", Code::ArrowUp),
            ("down", Code::ArrowDown),
            ("arrowdown", Code::ArrowDown),
            ("left", Code::ArrowLeft),
            ("arrowleft", Code::ArrowLeft),
            ("right", Code::ArrowRight),
            ("arrowright", Code::ArrowRight),
        ];
        for (token, code) in cases {
            assert_eq!(parse(token), hk(Modifiers::empty(), code), "{token}");
        }
    }

    #[test]
    fn editing_keys() {
        let cases = [
            ("backspace", Code::Backspace),
            ("delete", Code::Delete),
            ("del", Code::Delete),
            ("home", Code::Home),
            ("end", Code::End),
            ("pageup", Code::PageUp),
            ("pgup", Code::PageUp),
            ("pagedown", Code::PageDown),
            ("pgdn", Code::PageDown),
        ];
        for (token, code) in cases {
            assert_eq!(parse(token), hk(Modifiers::empty(), code), "{token}");
        }
    }

    #[test]
    fn punctuation_keys() {
        let cases = [
            ("-", Code::Minus),
            ("=", Code::Equal),
            ("[", Code::BracketLeft),
            ("]", Code::BracketRight),
            (";", Code::Semicolon),
            ("'", Code::Quote),
            (",", Code::Comma),
            (".", Code::Period),
            ("/", Code::Slash),
            ("`", Code::Backquote),
            ("\\", Code::Backslash),
        ];
        for (token, code) in cases {
            assert_eq!(parse(token), hk(Modifiers::empty(), code), "{token}");
            assert_eq!(
                parse(&format!("ctrl+{token}")),
                hk(Modifiers::CONTROL, code),
                "ctrl+{token}"
            );
        }
    }

    #[test]
    fn case_and_whitespace_are_ignored() {
        let expected = hk(Modifiers::CONTROL | Modifiers::SHIFT, Code::Space);
        assert_eq!(parse(" Ctrl + Shift + Space "), expected);
        assert_eq!(parse("CTRL+SHIFT+SPACE"), expected);
        assert_eq!(parse("\tctrl+shift+space\n"), expected);
        // Empty segments from doubled or trailing separators are skipped.
        assert_eq!(parse("ctrl++shift+space+"), expected);
    }

    #[test]
    fn last_key_wins_when_spec_names_two_keys() {
        // Documents current behaviour: a second key replaces the first
        // instead of being rejected.
        assert_eq!(parse("ctrl+a+b"), hk(Modifiers::CONTROL, Code::KeyB));
        assert_eq!(parse("space+f1"), hk(Modifiers::empty(), Code::F1));
    }

    #[test]
    fn missing_key_error_mentions_example() {
        for spec in ["ctrl+shift", "cmd", "", "  ", "+"] {
            let msg = err(spec);
            assert!(msg.contains("missing a key"), "{spec:?}: {msg}");
            assert!(msg.contains("ctrl+shift+space"), "{spec:?}: {msg}");
        }
        assert!(err("ctrl+shift").contains("`ctrl+shift`"));
    }

    #[test]
    fn unknown_token_error_names_the_token() {
        let msg = err("ctrl+hyper+space");
        assert!(msg.contains("unsupported hotkey token"), "{msg}");
        assert!(msg.contains("hyper"), "{msg}");

        let msg = err("ctrl+!");
        assert!(msg.contains("unsupported hotkey key"), "{msg}");
        assert!(msg.contains('!'), "{msg}");

        // Multi-byte single characters are reported as tokens, not keys.
        assert!(err("é").contains("unsupported hotkey token: é"));
    }

    #[test]
    fn equivalent_specs_share_an_id() {
        assert_eq!(parse("ctrl+shift+p").id(), parse("shift+control+P").id());
        assert_eq!(parse("cmd+space").id(), parse("Super + Space").id());
        assert_eq!(parse("alt+up").id(), parse("option+arrowup").id());
        assert_eq!(parse("ctrl+shift+p"), parse("shift+control+P"));
    }

    #[test]
    fn different_chords_have_different_ids() {
        let chords = [
            "ctrl+p",
            "ctrl+shift+p",
            "ctrl+o",
            "cmd+p",
            "f5",
            "shift+f5",
            "ctrl+up",
            "ctrl+down",
            "ctrl+-",
            "ctrl+=",
        ];
        let ids: Vec<u32> = chords.iter().map(|s| parse(s).id()).collect();
        for (i, a) in ids.iter().enumerate() {
            for (j, b) in ids.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "{} vs {}", chords[i], chords[j]);
                }
            }
        }
    }

    #[test]
    fn native_editing_chords_are_reserved_exactly() {
        for spec in ["cmd+z", "shift+cmd+z", "cmd+c", "cmd+x", "cmd+v"] {
            assert!(is_reserved_native(&parse(spec)), "{spec}");
        }
        for spec in ["cmd+a", "shift+cmd+c", "ctrl+z", "cmd+escape"] {
            assert!(!is_reserved_native(&parse(spec)), "{spec}");
        }
    }

    #[test]
    fn fixture_retries_only_transient_bindings_and_keeps_successes() {
        let retry_id = parse("cmd+a").id();
        let success_id = parse("cmd+b").id();
        let attempts = std::cell::RefCell::new(Vec::new());
        let mut fail_once = true;
        let mut register = |hotkey: HotKey| {
            attempts.borrow_mut().push(hotkey.id());
            if hotkey.id() == retry_id && fail_once {
                fail_once = false;
                Err("temporarily unavailable".to_string())
            } else {
                Ok(())
            }
        };
        let mut map = HashMap::new();
        let mut claimed = HashSet::new();
        let mut pending = Vec::new();

        register_binding(
            &mut map,
            &mut claimed,
            &mut pending,
            "cmd+a",
            HotkeyAction::Command("first".to_string()),
            "command `first`",
            &mut register,
        );
        register_binding(
            &mut map,
            &mut claimed,
            &mut pending,
            "cmd+a",
            HotkeyAction::Command("duplicate".to_string()),
            "command `duplicate`",
            &mut register,
        );
        register_binding(
            &mut map,
            &mut claimed,
            &mut pending,
            "cmd+b",
            HotkeyAction::Command("second".to_string()),
            "command `second`",
            &mut register,
        );
        register_binding(
            &mut map,
            &mut claimed,
            &mut pending,
            "cmd+z",
            HotkeyAction::Command("reserved".to_string()),
            "command `reserved`",
            &mut register,
        );
        register_binding(
            &mut map,
            &mut claimed,
            &mut pending,
            "ctrl+hyper+space",
            HotkeyAction::Command("invalid".to_string()),
            "command `invalid`",
            &mut register,
        );

        assert_eq!(&*attempts.borrow(), &[retry_id, success_id]);
        assert_eq!(pending.len(), 1, "only the transient binding is pending");
        assert!(
            map.contains_key(&success_id),
            "successful binding remains active"
        );

        let (remaining, escape_restored) =
            retry_pending_bindings(&mut map, &mut pending, &mut register);
        assert_eq!(remaining, 0);
        assert!(!escape_restored);
        assert!(pending.is_empty());
        assert!(
            map.contains_key(&retry_id),
            "retry adds the missing binding"
        );
        assert!(
            map.contains_key(&success_id),
            "retry preserves prior success"
        );
        assert_eq!(&*attempts.borrow(), &[retry_id, success_id, retry_id]);
    }

    #[test]
    fn fixture_retries_escape_without_losing_command_bindings() {
        let escape = hk(Modifiers::empty(), Code::Escape);
        let command = parse("cmd+b");
        let mut map = HashMap::from([(
            command.id(),
            HotkeyAction::Command("already-restored".to_string()),
        )]);
        let mut pending = vec![PendingBinding {
            hotkey: escape,
            action: HotkeyAction::Cancel,
            spec: escape.to_string(),
            label: "Escape cancellation".to_string(),
        }];
        let mut attempts = 0;
        let mut register = |_hotkey: HotKey| {
            attempts += 1;
            if attempts == 1 {
                Err("temporarily unavailable".to_string())
            } else {
                Ok(())
            }
        };

        let (remaining, escape_restored) =
            retry_pending_bindings(&mut map, &mut pending, &mut register);
        assert_eq!(remaining, 1);
        assert!(!escape_restored);
        assert!(map.contains_key(&command.id()));

        let (remaining, escape_restored) =
            retry_pending_bindings(&mut map, &mut pending, &mut register);
        assert_eq!(remaining, 0);
        assert!(escape_restored);
        assert!(matches!(map.get(&escape.id()), Some(HotkeyAction::Cancel)));
        assert!(map.contains_key(&command.id()));
    }

    #[test]
    fn fixture_preserves_requested_escape_across_full_reregister_after_failure() {
        let escape = hk(Modifiers::empty(), Code::Escape);
        let cancel_requested = AtomicBool::new(true);
        let cancel_enabled = AtomicBool::new(false);
        let mut map = HashMap::new();
        let mut pending = Vec::new();
        let mut attempts = 0;
        let mut register = |_hotkey: HotKey| {
            attempts += 1;
            if attempts < 3 {
                Err("temporarily unavailable".to_string())
            } else {
                Ok(())
            }
        };

        // The active run requested Escape, but its first native restore failed.
        register_cancel_binding(
            &mut map,
            &mut pending,
            escape,
            &mut register,
            &cancel_enabled,
        );
        assert!(cancellation_is_requested(
            cancel_requested.load(Ordering::Acquire),
            &pending
        ));
        assert!(!cancel_enabled.load(Ordering::Acquire));

        // A config change rebuilds the manager. It must capture the desired
        // state before clearing the old pending queue and retry Escape in the
        // replacement manager.
        let cancel_was_requested =
            cancellation_is_requested(cancel_requested.load(Ordering::Acquire), &pending);
        pending.clear();
        map.clear();
        cancel_enabled.store(false, Ordering::Release);
        assert!(cancel_was_requested);
        if cancel_was_requested {
            register_cancel_binding(
                &mut map,
                &mut pending,
                escape,
                &mut register,
                &cancel_enabled,
            );
        }
        assert_eq!(pending.len(), 1, "failed Escape remains retryable");
        assert!(!cancel_enabled.load(Ordering::Acquire));

        let (remaining, escape_restored) =
            retry_pending_bindings(&mut map, &mut pending, &mut register);
        if escape_restored {
            cancel_enabled.store(true, Ordering::Release);
        }
        assert_eq!(remaining, 0);
        assert!(escape_restored);
        assert!(cancel_requested.load(Ordering::Acquire));
        assert!(cancel_enabled.load(Ordering::Acquire));
        assert!(matches!(map.get(&escape.id()), Some(HotkeyAction::Cancel)));
    }

    #[test]
    fn handler_discards_event_waiting_across_registration_transition() {
        let shared = Arc::new(SharedHotkeys {
            registration_lock: Mutex::new(()),
            registration_epoch: AtomicU64::new(0),
            registration_active: AtomicU64::new(0),
            by_id: Mutex::new(HashMap::new()),
            pending_bindings: Mutex::new(Vec::new()),
            pending: Mutex::new(None),
            wake: Mutex::new(None),
            cancel_requested: AtomicBool::new(false),
            cancel_enabled: AtomicBool::new(false),
        });
        let hotkey = parse("cmd+a");
        shared
            .by_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(hotkey.id(), HotkeyAction::Command("old".to_string()));
        let captured_epoch = shared.registration_epoch.load(Ordering::Acquire);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handler_shared = Arc::clone(&shared);
        let handler_barrier = Arc::clone(&barrier);
        let handler = std::thread::spawn(move || {
            handler_barrier.wait();
            handler_shared.handle_at_epoch(
                GlobalHotKeyEvent {
                    id: hotkey.id(),
                    state: HotKeyState::Pressed,
                },
                captured_epoch,
            );
        });

        let registration = shared
            .registration_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        barrier.wait();
        // A transition starts and completes while the handler waits for the
        // registration lock. Its captured epoch is now stale, even though
        // the replacement map may reuse the same native id.
        shared.registration_active.fetch_add(1, Ordering::AcqRel);
        shared.registration_epoch.fetch_add(1, Ordering::AcqRel);
        shared.registration_epoch.fetch_add(1, Ordering::Release);
        shared.registration_active.fetch_sub(1, Ordering::Release);
        drop(registration);
        handler.join().expect("handler thread should finish");

        assert!(shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none());
        assert!(shared
            .by_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&hotkey.id()));
    }
}
