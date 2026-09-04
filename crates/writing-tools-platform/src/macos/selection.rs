//! Selection read/replace via Accessibility, with Cmd+C / Cmd+V clipboard fallback.
//!
//! Tradeoff: AX `AXSelectedText` set is preferred when the focused element supports
//! it. Many apps ignore setValue; the fallback saves the clipboard, pastes the
//! result with Cmd+V, then restores the previous clipboard after a short delay.
//! Clipboard restore can race if the user copies something else in that window.

#![allow(deprecated, unexpected_cfgs)]

use anyhow::{anyhow, bail, Context, Result};
use arboard::Clipboard;
use async_trait::async_trait;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, KeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc::{class, msg_send, sel, sel_impl};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tracing::{debug, warn};

use accessibility::attribute::AXAttribute;
use accessibility::ui_element::AXUIElement;

use crate::{SelectionService, SelectionSnapshot};

/// True when this process is trusted for Accessibility APIs.
pub fn accessibility_trusted() -> bool {
    unsafe { accessibility_sys::AXIsProcessTrusted() }
}

/// Prompt macOS to show the Accessibility permission dialog (best-effort).
pub fn prompt_accessibility() {
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString as CFStr;

    let key = CFStr::from_static_string("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::true_value();
    let opts: CFDictionary<CFStr, CFBoolean> = CFDictionary::from_CFType_pairs(&[(key, value)]);
    unsafe {
        let _ = accessibility_sys::AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef());
    }
}

fn attr_typed<T>(name: &'static str) -> AXAttribute<T> {
    // `AXAttribute::new` is typed as CFType; PhantomData is ZST so this is layout-identical.
    let untyped = AXAttribute::new(&CFString::from_static_string(name));
    unsafe { std::mem::transmute::<AXAttribute<core_foundation::base::CFType>, AXAttribute<T>>(untyped) }
}

fn focused_element() -> Result<AXUIElement> {
    let system = AXUIElement::system_wide();
    system
        .attribute(&attr_typed::<AXUIElement>("AXFocusedUIElement"))
        .map_err(|e| anyhow!("AXFocusedUIElement: {e}"))
}

fn read_ax_selected_text(element: &AXUIElement) -> Result<String> {
    let text: CFString = element
        .attribute(&attr_typed::<CFString>("AXSelectedText"))
        .map_err(|e| anyhow!("AXSelectedText: {e}"))?;
    Ok(text.to_string())
}

fn set_ax_selected_text(element: &AXUIElement, text: &str) -> Result<()> {
    element
        .set_attribute(&attr_typed::<CFString>("AXSelectedText"), CFString::new(text))
        .map_err(|e| anyhow!("set AXSelectedText: {e}"))
}

fn frontmost_app_name() -> Option<String> {
    unsafe {
        let workspace: cocoa::base::id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: cocoa::base::id = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return None;
        }
        let name: cocoa::base::id = msg_send![app, localizedName];
        if name.is_null() {
            return None;
        }
        let utf8: *const std::os::raw::c_char = msg_send![name, UTF8String];
        if utf8.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}

/// Process id of the frontmost app (call before showing our UI).
pub fn frontmost_pid() -> Option<i32> {
    unsafe {
        let workspace: cocoa::base::id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: cocoa::base::id = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return None;
        }
        let pid: i32 = msg_send![app, processIdentifier];
        Some(pid)
    }
}

/// Re-activate another app so selection/paste targets it, not our picker.
pub fn activate_pid(pid: i32) -> Result<()> {
    unsafe {
        let app: cocoa::base::id =
            msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid];
        if app.is_null() {
            bail!("no running application for pid {pid}");
        }
        // NSApplicationActivateIgnoringOtherApps = 1 << 1
        let ok: bool = msg_send![app, activateWithOptions: 1u64 << 1];
        if !ok {
            warn!("activateWithOptions returned false for pid {pid}");
        }
    }
    thread::sleep(Duration::from_millis(80));
    Ok(())
}

fn post_key(keycode: u16, flags: CGEventFlags, key_down: bool) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("CGEventSource::new failed"))?;
    let event = CGEvent::new_keyboard_event(source, keycode, key_down)
        .map_err(|_| anyhow!("CGEvent::new_keyboard_event failed"))?;
    event.set_flags(flags);
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn cmd_keystroke(keycode: u16) -> Result<()> {
    let flags = CGEventFlags::CGEventFlagCommand;
    post_key(keycode, flags, true)?;
    thread::sleep(Duration::from_millis(15));
    post_key(keycode, flags, false)?;
    thread::sleep(Duration::from_millis(40));
    Ok(())
}

fn clipboard_copy() -> Result<()> {
    cmd_keystroke(KeyCode::ANSI_C)
}

fn clipboard_paste() -> Result<()> {
    cmd_keystroke(KeyCode::ANSI_V)
}

fn clip_get(clip: &Mutex<Clipboard>) -> Result<Option<String>> {
    let mut c = clip.lock().unwrap_or_else(|e| e.into_inner());
    match c.get_text() {
        Ok(t) => Ok(Some(t)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn clip_set(clip: &Mutex<Clipboard>, text: &str) -> Result<()> {
    let mut c = clip.lock().unwrap_or_else(|e| e.into_inner());
    c.set_text(text.to_string())?;
    Ok(())
}

pub struct MacosSelection {
    clipboard: Mutex<Clipboard>,
}

impl MacosSelection {
    pub fn new() -> Result<Self> {
        Ok(Self {
            clipboard: Mutex::new(Clipboard::new().context("open clipboard")?),
        })
    }

    fn read_via_clipboard_fallback(&self) -> Result<Option<String>> {
        let previous = clip_get(&self.clipboard)?;
        clipboard_copy()?;
        thread::sleep(Duration::from_millis(60));
        let copied = clip_get(&self.clipboard)?;
        if let Some(prev) = previous.as_ref() {
            let _ = clip_set(&self.clipboard, prev);
        }
        Ok(copied.filter(|t| !t.is_empty()))
    }

    fn replace_via_clipboard_fallback(&self, text: &str) -> Result<()> {
        let previous = clip_get(&self.clipboard)?;
        clip_set(&self.clipboard, text)?;
        thread::sleep(Duration::from_millis(40));
        clipboard_paste()?;
        // Delayed restore so the target app can consume the paste.
        let prev = previous;
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(350));
            if let Some(p) = prev {
                if let Ok(mut clip) = Clipboard::new() {
                    let _ = clip.set_text(p);
                }
            }
        });
        Ok(())
    }
}

impl Default for MacosSelection {
    fn default() -> Self {
        Self::new().expect("clipboard")
    }
}

#[async_trait]
impl SelectionService for MacosSelection {
    async fn read_selection(&self) -> Result<Option<SelectionSnapshot>> {
        if !accessibility_trusted() {
            bail!(
                "Accessibility permission missing. Enable Writing Tools (or the Terminal/binary \
                 you launched) under System Settings → Privacy & Security → Accessibility."
            );
        }

        let app_name = frontmost_app_name();

        match focused_element().and_then(|el| read_ax_selected_text(&el)) {
            Ok(text) if !text.is_empty() => {
                debug!(len = text.len(), "read selection via AX");
                return Ok(Some(SelectionSnapshot { text, app_name }));
            }
            Ok(_) => debug!("AX selected text empty; trying clipboard fallback"),
            Err(e) => debug!("AX read failed ({e}); trying clipboard fallback"),
        }

        let text = self
            .read_via_clipboard_fallback()
            .context("clipboard selection fallback")?;
        Ok(text.map(|text| SelectionSnapshot { text, app_name }))
    }

    async fn replace_selection(&self, text: &str) -> Result<()> {
        if !accessibility_trusted() {
            bail!(
                "Accessibility permission missing. Enable Writing Tools (or the Terminal/binary \
                 you launched) under System Settings → Privacy & Security → Accessibility."
            );
        }

        if let Ok(el) = focused_element() {
            match set_ax_selected_text(&el, text) {
                Ok(()) => {
                    debug!(len = text.len(), "replaced selection via AX");
                    return Ok(());
                }
                Err(e) => debug!("AX replace failed ({e}); using clipboard paste fallback"),
            }
        }

        self.replace_via_clipboard_fallback(text)
            .context("clipboard replace fallback")
    }
}
