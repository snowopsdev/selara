use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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

struct SharedHotkeys {
    /// hotkey id → action
    by_id: Mutex<HashMap<u32, HotkeyAction>>,
    pending: Mutex<Option<HotkeyAction>>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    cancel_enabled: AtomicBool,
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
                by_id: Mutex::new(HashMap::new()),
                pending: Mutex::new(None),
                wake: Mutex::new(None),
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
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Enable or disable the Escape binding used to cancel the active run.
    /// The binding is registered only while enabled, so idle applications keep
    /// their native Escape behavior.
    pub fn set_cancel_enabled(&self, enabled: bool) -> Result<()> {
        let cancel_hk = HotKey::new(None, Code::Escape);
        let manager = self.manager.lock().unwrap_or_else(|e| e.into_inner());
        let Some(manager) = manager.as_ref() else {
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
            let mut pending = self
                .shared
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if matches!(pending.as_ref(), Some(HotkeyAction::Cancel)) {
                pending.take();
            }
        }
        Ok(())
    }

    /// Unregister everything and register the custom-instruction and command
    /// hotkeys. `command_hotkeys` is `(command_id, hotkey_spec)`.
    ///
    /// Invalid, reserved, duplicate, or unavailable individual bindings are
    /// reported and skipped so one bad command cannot disable the others.
    pub fn reregister_all(
        &self,
        custom_instruction: &str,
        command_hotkeys: &[(String, String)],
    ) -> Result<()> {
        let cancel_was_enabled = self.shared.cancel_enabled.load(Ordering::Acquire);
        // Drop the previous manager before creating the replacement. macOS
        // rejects duplicate registrations while the old manager is alive.
        let old_manager = self
            .manager
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        drop(old_manager);
        self.shared.cancel_enabled.store(false, Ordering::Release);
        let manager =
            GlobalHotKeyManager::new().context("create GlobalHotKeyManager (main thread)")?;

        let mut map = HashMap::new();

        register_binding(
            &manager,
            &mut map,
            custom_instruction,
            HotkeyAction::CustomInstruction,
            "custom instruction",
        );

        for (cmd_id, spec) in command_hotkeys {
            register_binding(
                &manager,
                &mut map,
                spec,
                HotkeyAction::Command(cmd_id.clone()),
                &format!("command `{cmd_id}`"),
            );
        }

        if cancel_was_enabled {
            let cancel_hk = HotKey::new(None, Code::Escape);
            match manager.register(cancel_hk) {
                Ok(()) => {
                    map.insert(cancel_hk.id(), HotkeyAction::Cancel);
                    self.shared.cancel_enabled.store(true, Ordering::Release);
                }
                Err(e) => warn!(error = %e, "could not restore Escape cancellation hotkey"),
            }
        }

        *self.shared.by_id.lock().unwrap_or_else(|e| e.into_inner()) = map;
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

fn is_reserved_native(hk: &HotKey) -> bool {
    let cmd = Modifiers::SUPER;
    match (hk.mods, hk.key) {
        (mods, Code::KeyZ) => mods == cmd || mods == (cmd | Modifiers::SHIFT),
        (mods, Code::KeyC | Code::KeyX | Code::KeyV) => mods == cmd,
        _ => false,
    }
}

fn register_binding(
    manager: &GlobalHotKeyManager,
    map: &mut HashMap<u32, HotkeyAction>,
    spec: &str,
    action: HotkeyAction,
    label: &str,
) {
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
    if map.contains_key(&hk.id()) {
        warn!(binding = %spec, target = %label, "ignoring duplicate hotkey");
        return;
    }
    if let Err(e) = manager.register(hk) {
        warn!(binding = %spec, target = %label, error = %e, "could not register hotkey");
        return;
    }
    map.insert(hk.id(), action);
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
        self.reregister_all(hotkey, &[])
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
}
