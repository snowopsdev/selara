//! macOS Accessibility + global-hotkey + clipboard backend.
//!
//! Requires Accessibility permission for selection read/replace and synthetic
//! Cmd+C / Cmd+V fallbacks. Create and register [`MacosHotkey`] on the main
//! thread (the same thread that runs the UI event loop), then call
//! [`MacosHotkey::poll`] each frame.

mod clipboard;
mod hotkey;
mod selection;

pub use clipboard::MacosClipboard;
pub use hotkey::{parse_hotkey, MacosHotkey};
pub use selection::{
    accessibility_trusted, activate_pid, frontmost_pid, prompt_accessibility, MacosSelection,
};

/// Convenience bundle of macOS platform services.
pub struct MacosPlatform {
    pub selection: MacosSelection,
    pub clipboard: MacosClipboard,
    pub hotkey: MacosHotkey,
}

impl MacosPlatform {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            selection: MacosSelection::new()?,
            clipboard: MacosClipboard::new()?,
            hotkey: MacosHotkey::new(),
        })
    }
}
