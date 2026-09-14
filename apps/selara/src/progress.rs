//! Native, nonactivating command progress, rendered with AppKit + ThinkingOrbsKit.
//! The Swift owner never becomes key; source selection and focus stay in Rust.

use std::ffi::{c_char, c_void, CString};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

unsafe extern "C" {
    fn selara_progress_create() -> *mut c_void;
    fn selara_progress_destroy(handle: *mut c_void);
    fn selara_progress_capture_anchor(handle: *mut c_void, source_pid: i32);
    fn selara_progress_show(handle: *mut c_void, title: *const c_char);
    fn selara_progress_hide(handle: *mut c_void);
    fn selara_progress_succeed(handle: *mut c_void);
    fn selara_progress_take_cancelled(handle: *mut c_void) -> bool;
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
        // appears. Swift stores a selection rect or the current pointer location.
        unsafe { selara_progress_capture_anchor(self.handle.as_ptr(), source_pid.unwrap_or(0)) }
    }

    pub fn show(&self, title: &str) {
        // Config labels may contain NUL; don't let one terminate a C string.
        let title = CString::new(title.replace('\0', " ")).expect("NULs removed");
        // SAFETY: live controller; Swift copies title before this call returns.
        unsafe {
            selara_progress_show(self.handle.as_ptr(), title.as_ptr());
        }
    }

    pub fn succeed(&self) {
        // SAFETY: main-thread access to the live controller. Delayed callbacks
        // are weak and generation-checked inside Swift.
        unsafe { selara_progress_succeed(self.handle.as_ptr()) }
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
