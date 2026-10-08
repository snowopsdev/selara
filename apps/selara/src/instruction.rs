//! Native custom-instruction popover (direction 2B), rendered with AppKit in
//! `native/InstructionPopover.swift`. Swift owns the panel and its typing;
//! Rust drains its event queue every frame and keeps the instruction history.

use std::ffi::{c_char, c_void, CStr, CString};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

use eframe::egui;

unsafe extern "C" {
    fn selara_instruction_create(
        wake: Option<extern "C" fn(*mut c_void)>,
        context: *mut c_void,
    ) -> *mut c_void;
    fn selara_instruction_destroy(handle: *mut c_void);
    fn selara_instruction_capture_anchor(handle: *mut c_void, source_pid: i32);
    fn selara_instruction_show(
        handle: *mut c_void,
        app: *const c_char,
        chars: u64,
        history: *const *const c_char,
        count: isize,
    );
    fn selara_instruction_hide(handle: *mut c_void);
    fn selara_instruction_is_visible(handle: *mut c_void) -> bool;
    fn selara_instruction_set_notice(handle: *mut c_void, text: *const c_char, success: bool);
    fn selara_instruction_take_event(handle: *mut c_void) -> *mut c_char;
}

/// What the popover asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstructionEvent {
    /// ↩ / Replace: run this instruction against the captured selection.
    Submit(String),
    /// ⌘S: save this instruction as a command; the popover stays open.
    Save(String),
    /// Esc / ⌘W: closed by the user; hand focus back to the source app.
    Cancel,
    /// Lost key focus (clicked elsewhere): closed, focus is already elsewhere.
    Dismiss,
}

/// Decode Swift's `kind` / `kind\ntext` encoding (`InstructionEvent.encoded`).
/// Unknown kinds and empty instructions are dropped.
pub fn decode_event(raw: &str) -> Option<InstructionEvent> {
    let (kind, text) = match raw.split_once('\n') {
        Some((kind, text)) => (kind, Some(text)),
        None => (raw, None),
    };
    let text = || {
        text.map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    };
    match kind {
        "submit" => text().map(InstructionEvent::Submit),
        "save" => text().map(InstructionEvent::Save),
        "cancel" => Some(InstructionEvent::Cancel),
        "dismiss" => Some(InstructionEvent::Dismiss),
        _ => None,
    }
}

/// C strings for the FFI: NULs are replaced so a label cannot truncate.
fn c_string(text: &str) -> CString {
    CString::new(text.replace('\0', " ")).expect("NULs removed")
}

extern "C" fn wake_egui(context: *mut c_void) {
    // SAFETY: `context` is the boxed Context owned by `InstructionPanel`,
    // which outlives the Swift controller (destroyed first in Drop).
    if let Some(ctx) = unsafe { context.cast::<egui::Context>().as_ref() } {
        ctx.request_repaint();
    }
}

pub struct InstructionPanel {
    handle: NonNull<c_void>,
    _wake: Box<egui::Context>,
    // AppKit access and destruction must stay on the creating main thread.
    _main_thread: PhantomData<Rc<()>>,
}

impl InstructionPanel {
    pub fn new(ctx: &egui::Context) -> Self {
        let wake = Box::new(ctx.clone());
        let context = (&*wake as *const egui::Context).cast_mut().cast::<c_void>();
        // SAFETY: called on the AppKit thread (ServeApp::new). Swift asserts
        // that and returns one retained controller; `context` stays alive in
        // `_wake` until after `selara_instruction_destroy`.
        let handle = NonNull::new(unsafe { selara_instruction_create(Some(wake_egui), context) })
            .expect("native instruction controller must be retained");
        Self {
            handle,
            _wake: wake,
            _main_thread: PhantomData,
        }
    }

    /// Record the selection's AX bounds (or the pointer) before any UI shows.
    pub fn capture_anchor(&self, source_pid: Option<i32>) {
        // SAFETY: main-thread, read-only AX geometry capture.
        unsafe { selara_instruction_capture_anchor(self.handle.as_ptr(), source_pid.unwrap_or(0)) }
    }

    /// Open the popover. `history` is newest first.
    pub fn show<'a>(
        &self,
        app: Option<&str>,
        chars: u64,
        history: impl IntoIterator<Item = &'a String>,
    ) {
        let app = app.map(c_string);
        let history: Vec<CString> = history.into_iter().map(|h| c_string(h)).collect();
        let pointers: Vec<*const c_char> = history.iter().map(|h| h.as_ptr()).collect();
        // SAFETY: live controller; every pointer outlives this call and Swift
        // copies the strings before returning.
        unsafe {
            selara_instruction_show(
                self.handle.as_ptr(),
                app.as_ref().map_or(std::ptr::null(), |a| a.as_ptr()),
                chars,
                pointers.as_ptr(),
                pointers.len() as isize,
            )
        }
    }

    pub fn hide(&self) {
        // SAFETY: main-thread access to the live controller.
        unsafe { selara_instruction_hide(self.handle.as_ptr()) }
    }

    pub fn is_visible(&self) -> bool {
        // SAFETY: main-thread read of the live controller.
        unsafe { selara_instruction_is_visible(self.handle.as_ptr()) }
    }

    /// Meta-row receipt; successes fade after the motion hold.
    pub fn set_notice(&self, text: &str, success: bool) {
        let text = c_string(text);
        // SAFETY: live controller; Swift copies the string before returning.
        unsafe { selara_instruction_set_notice(self.handle.as_ptr(), text.as_ptr(), success) }
    }

    /// Next queued event, if any. Call until `None` each frame.
    pub fn take_event(&self) -> Option<InstructionEvent> {
        loop {
            // SAFETY: main-thread access; Swift returns a strdup'd string or NULL.
            let raw = unsafe { selara_instruction_take_event(self.handle.as_ptr()) };
            if raw.is_null() {
                return None;
            }
            // SAFETY: non-null, NUL-terminated, malloc'd by strdup; read once
            // and released with the matching allocator.
            let text = unsafe { CStr::from_ptr(raw) }
                .to_string_lossy()
                .into_owned();
            unsafe { libc::free(raw.cast()) };
            if let Some(event) = decode_event(&text) {
                return Some(event);
            }
            tracing::warn!("ignoring malformed instruction popover event");
        }
    }
}

impl Drop for InstructionPanel {
    fn drop(&mut self) {
        // SAFETY: exactly one owner releases the retained Swift handle, before
        // `_wake` (the callback context) is dropped.
        unsafe { selara_instruction_destroy(self.handle.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_event, InstructionEvent};

    #[test]
    fn decodes_submit_and_save_with_multiline_text() {
        assert_eq!(
            decode_event("submit\nMake it shorter. Keep\nthe date."),
            Some(InstructionEvent::Submit(
                "Make it shorter. Keep\nthe date.".into()
            ))
        );
        assert_eq!(
            decode_event("save\n  Make it warmer.  "),
            Some(InstructionEvent::Save("Make it warmer.".into()))
        );
    }

    #[test]
    fn decodes_close_events() {
        assert_eq!(decode_event("cancel"), Some(InstructionEvent::Cancel));
        assert_eq!(decode_event("dismiss"), Some(InstructionEvent::Dismiss));
    }

    #[test]
    fn drops_empty_and_unknown_events() {
        assert_eq!(decode_event("submit\n   "), None);
        assert_eq!(decode_event("submit"), None);
        assert_eq!(decode_event("save\n"), None);
        assert_eq!(decode_event("launch\nrockets"), None);
        assert_eq!(decode_event(""), None);
    }

    #[test]
    fn only_the_first_newline_separates_the_kind() {
        assert_eq!(
            decode_event("submit\ncancel\nsave"),
            Some(InstructionEvent::Submit("cancel\nsave".into()))
        );
    }
}
