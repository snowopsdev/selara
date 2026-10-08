//! Native, nonactivating command feedback, rendered with AppKit + ThinkingOrbsKit.
//!
//! Every run shows the Ink Sweep over the selected lines when Accessibility
//! reports plausible per-line bounds, and the orb otherwise. Commands with
//! `review = true` show the Ghost Diff card instead; it is the only surface
//! that becomes key (a non-activating panel), and it reports the user's
//! choice through [`ProgressPanel::take_review_action`]. Source selection,
//! focus, and replacement stay in Rust.

use std::ffi::{c_char, c_void, CString};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::review::ReviewAction;

unsafe extern "C" {
    fn selara_progress_create() -> *mut c_void;
    fn selara_progress_destroy(handle: *mut c_void);
    fn selara_progress_capture_anchor(handle: *mut c_void, source_pid: i32);
    fn selara_progress_show(handle: *mut c_void, title: *const c_char);
    fn selara_progress_show_review(handle: *mut c_void, title: *const c_char, model: *const c_char);
    fn selara_progress_review_result(handle: *mut c_void, json: *const c_char) -> bool;
    fn selara_progress_close_review(handle: *mut c_void, copied: bool);
    fn selara_progress_take_review_action(handle: *mut c_void) -> i32;
    fn selara_progress_hide(handle: *mut c_void);
    fn selara_progress_succeed_range(handle: *mut c_void, location: i64, length: i64);
    fn selara_progress_take_cancelled(handle: *mut c_void) -> bool;
}

/// Config labels and model names may contain NUL; don't let one end a C string.
fn c_text(text: &str) -> CString {
    CString::new(text.replace('\0', " ")).expect("NULs removed")
}

pub struct ProgressPanel {
    handle: NonNull<c_void>,
    // AppKit access and destruction must stay on the creating main thread.
    _main_thread: PhantomData<Rc<()>>,
}

impl ProgressPanel {
    pub fn new() -> Self {
        // SAFETY: ServeApp creates this on the AppKit thread. Swift asserts
        // that condition and returns one retained, non-null controller.
        let handle = NonNull::new(unsafe { selara_progress_create() })
            .expect("native progress controller must be retained");
        Self {
            handle,
            _main_thread: PhantomData,
        }
    }

    pub fn capture_anchor(&self, source_pid: Option<i32>) {
        // SAFETY: main-thread, read-only geometry capture before any Selara UI
        // appears. Swift stores per-line selection bounds (when plausible), a
        // selection rect, or the current pointer location.
        unsafe { selara_progress_capture_anchor(self.handle.as_ptr(), source_pid.unwrap_or(0)) }
    }

    /// Ink Sweep (or the orb fallback) after the fast-command delay.
    pub fn show(&self, title: &str) {
        let title = c_text(title);
        // SAFETY: live controller; Swift copies title before this call returns.
        unsafe {
            selara_progress_show(self.handle.as_ptr(), title.as_ptr());
        }
    }

    /// Ghost Diff skeleton headed “title · model”. Called again for another
    /// take, it switches the visible card back to its skeleton.
    pub fn show_review(&self, title: &str, model: &str) {
        let (title, model) = (c_text(title), c_text(model));
        // SAFETY: live controller; Swift copies both strings before returning.
        unsafe {
            selara_progress_show_review(self.handle.as_ptr(), title.as_ptr(), model.as_ptr());
        }
    }

    /// Show the diff (JSON from [`crate::review::diff_payload`]) and give the
    /// card key focus. False when the card is no longer showing.
    pub fn review_result(&self, payload: &str) -> bool {
        let payload = c_text(payload);
        // SAFETY: live controller; Swift decodes the JSON before returning.
        unsafe { selara_progress_review_result(self.handle.as_ptr(), payload.as_ptr()) }
    }

    /// Fade the card out without replacing; `copied` leaves a “Copied” receipt.
    pub fn close_review(&self, copied: bool) {
        // SAFETY: main-thread access; also drops the card's key status.
        unsafe { selara_progress_close_review(self.handle.as_ptr(), copied) }
    }

    pub fn take_review_action(&self) -> Option<ReviewAction> {
        // SAFETY: main-thread access; reads and clears the card's last key.
        ReviewAction::from_code(unsafe { selara_progress_take_review_action(self.handle.as_ptr()) })
    }

    /// Verified replacement. `replaced` is the new text's AX range
    /// `(location, utf16_length)` for the afterglow, when known.
    pub fn succeed(&self, replaced: Option<(i64, i64)>) {
        let (location, length) = replaced.unwrap_or((-1, -1));
        // SAFETY: main-thread access to the live controller. Delayed callbacks
        // are weak and generation-checked inside Swift.
        unsafe { selara_progress_succeed_range(self.handle.as_ptr(), location, length) }
    }

    pub fn hide(&self) {
        // SAFETY: main-thread access; also invalidates queued animation frames.
        unsafe { selara_progress_hide(self.handle.as_ptr()) }
    }

    pub fn take_cancelled(&self) -> bool {
        // SAFETY: main-thread access; reads and clears the native button flag.
        unsafe { selara_progress_take_cancelled(self.handle.as_ptr()) }
    }
}

impl Drop for ProgressPanel {
    fn drop(&mut self) {
        // SAFETY: exactly one owner releases the retained Swift handle. !Send
        // prevents moving it off the AppKit thread; pending callbacks are weak.
        unsafe { selara_progress_destroy(self.handle.as_ptr()) }
    }
}
