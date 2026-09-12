//! Selection read/replace via Accessibility, with Cmd+C / Cmd+V clipboard fallback.
//!
//! AX `AXSelectedText` is used to read and verify the target. Replacement always
//! uses the target app's normal Cmd+V path; the fallback snapshots the whole
//! pasteboard (every item and every type, not just text) and restores it only
//! after ownership and replacement verification succeed.
//!
//! Replace must run **after** our UI hides and the source app is frontmost again.
//! The delayed restore compares `NSPasteboard.changeCount` against the value
//! `-[NSPasteboard clearContents]` returned for our own write — one call, so no
//! other process's write can be mistaken for ours — and additionally checks that
//! the pasteboard still holds the text we put there. Either guard failing leaves
//! the pasteboard alone.

#![allow(deprecated)]

use anyhow::{anyhow, bail, Context, Result};
use arboard::Clipboard;
use async_trait::async_trait;
use cocoa::foundation::{NSPoint, NSRect};
use core_foundation::base::{CFRange, CFTypeRef, TCFType};
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
    unsafe {
        std::mem::transmute::<AXAttribute<core_foundation::base::CFType>, AXAttribute<T>>(untyped)
    }
}

fn focused_element() -> Result<AXUIElement> {
    let system = AXUIElement::system_wide();
    system
        .attribute(&attr_typed::<AXUIElement>("AXFocusedUIElement"))
        .map_err(|e| anyhow!("AXFocusedUIElement: {e}"))
}

fn focused_element_for_pid(pid: i32) -> Result<AXUIElement> {
    let app = AXUIElement::application(pid);
    app.attribute(&attr_typed::<AXUIElement>("AXFocusedUIElement"))
        .map_err(|e| anyhow!("AXFocusedUIElement for pid {pid}: {e}"))
}

fn focused_window_for_pid(pid: i32) -> Result<AXUIElement> {
    let app = AXUIElement::application(pid);
    app.attribute(&attr_typed::<AXUIElement>("AXFocusedWindow"))
        .map_err(|e| anyhow!("AXFocusedWindow for pid {pid}: {e}"))
}

fn read_ax_selected_text(element: &AXUIElement) -> Result<String> {
    let text: CFString = element
        .attribute(&attr_typed::<CFString>("AXSelectedText"))
        .map_err(|e| anyhow!("AXSelectedText: {e}"))?;
    Ok(text.to_string())
}

fn read_ax_value_text(element: &AXUIElement) -> Result<String> {
    let text: CFString = element
        .attribute(&attr_typed::<CFString>("AXValue"))
        .map_err(|e| anyhow!("AXValue: {e}"))?;
    Ok(text.to_string())
}

fn replace_utf16_range(value: &str, range: (i64, i64), replacement: &str) -> Option<String> {
    let (location, length) = range;
    if location < 0 || length < 0 {
        return None;
    }
    let mut units: Vec<u16> = value.encode_utf16().collect();
    let start = location as usize;
    let end = start.checked_add(length as usize)?;
    if end > units.len() {
        return None;
    }
    units.splice(start..end, replacement.encode_utf16());
    String::from_utf16(&units).ok()
}

fn read_ax_selected_range(element: &AXUIElement) -> Option<(i64, i64)> {
    let value: core_foundation::base::CFType = element
        .attribute(&attr_typed::<core_foundation::base::CFType>(
            "AXSelectedTextRange",
        ))
        .ok()?;
    let ax_ref = value.as_CFTypeRef() as accessibility_sys::AXValueRef;
    if ax_ref.is_null() {
        return None;
    }
    unsafe {
        if accessibility_sys::AXValueGetType(ax_ref) != accessibility_sys::kAXValueTypeCFRange {
            return None;
        }
        let mut range = CFRange {
            location: 0,
            length: 0,
        };
        let ok = accessibility_sys::AXValueGetValue(
            ax_ref,
            accessibility_sys::kAXValueTypeCFRange,
            &mut range as *mut _ as *mut _,
        );
        if !ok {
            return None;
        }
        Some((range.location as i64, range.length as i64))
    }
}

/// Localized name of the frontmost app (`NSRunningApplication.localizedName`).
/// Call before showing our UI so it does not report Selara itself.
pub fn frontmost_app_name() -> Option<String> {
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
        Some(
            std::ffi::CStr::from_ptr(utf8)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// Bundle identifier of the frontmost app (`NSRunningApplication.bundleIdentifier`),
/// e.g. `com.apple.Terminal`. `None` for unbundled processes. Call before
/// showing our UI so it does not report Selara itself.
pub fn frontmost_bundle_id() -> Option<String> {
    unsafe {
        let workspace: cocoa::base::id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: cocoa::base::id = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return None;
        }
        let bundle: cocoa::base::id = msg_send![app, bundleIdentifier];
        if bundle.is_null() {
            return None;
        }
        let utf8: *const std::os::raw::c_char = msg_send![bundle, UTF8String];
        if utf8.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(utf8)
                .to_string_lossy()
                .into_owned(),
        )
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

/// Re-activate another app so selection/paste targets it, not Selara's UI.
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
    // Activation is async; give the target time to become key.
    thread::sleep(Duration::from_millis(220));
    Ok(())
}

/// `(frame, visibleFrame)` of every attached display, in AppKit's global
/// bottom-left-origin point space. Index 0 is the primary display (the one
/// with the menu bar, whose frame origin is `(0, 0)`).
fn screen_frames() -> Vec<(NSRect, NSRect)> {
    use objc::rc::autoreleasepool;
    // SAFETY: plain AppKit getters on the shared NSScreen list; every object
    // is null-checked and only by-value structs escape the autorelease pool.
    autoreleasepool(|| unsafe {
        let screens: cocoa::base::id = msg_send![class!(NSScreen), screens];
        if screens.is_null() {
            return Vec::new();
        }
        let count: usize = msg_send![screens, count];
        (0..count)
            .filter_map(|i| {
                let screen: cocoa::base::id = msg_send![screens, objectAtIndex: i];
                if screen.is_null() {
                    return None;
                }
                let frame: NSRect = msg_send![screen, frame];
                let visible: NSRect = msg_send![screen, visibleFrame];
                Some((frame, visible))
            })
            .collect()
    })
}

/// Height of the primary display. AppKit's global y axis is flipped around it
/// (not around whichever display a point happens to be on), which is also how
/// winit maps a top-left `OuterPosition` back to AppKit coordinates.
fn primary_height(screens: &[(NSRect, NSRect)]) -> Option<f64> {
    screens
        .iter()
        .find(|(frame, _)| frame.origin.x == 0.0 && frame.origin.y == 0.0)
        .or(screens.first())
        .map(|(frame, _)| frame.size.height)
}

fn rect_contains(rect: &NSRect, x: f64, y: f64) -> bool {
    let (x0, y0) = (rect.origin.x, rect.origin.y);
    x >= x0 && x <= x0 + rect.size.width && y >= y0 && y <= y0 + rect.size.height
}

/// Flip a y coordinate between AppKit's bottom-left origin and a top-left
/// origin. The mapping is its own inverse.
fn flip_y(primary_height: f64, y: f64) -> f64 {
    primary_height - y
}

/// Convert a bottom-left-origin `(x, y, w, h)` rect to top-left origin: same
/// size, origin moved from the bottom edge to the top edge.
fn rect_to_top_left(
    primary_height: f64,
    (x, y, w, h): (f64, f64, f64, f64),
) -> (f64, f64, f64, f64) {
    (x, primary_height - (y + h), w, h)
}

/// Current mouse position in top-left-origin screen points, the space
/// `ViewportCommand::OuterPosition` uses. `None` when no display contains the
/// cursor (a display was just unplugged, or AppKit has no screens yet).
pub fn mouse_location() -> Option<(f64, f64)> {
    let screens = screen_frames();
    let primary_h = primary_height(&screens)?;
    // SAFETY: argument-less class method returning a plain C struct by value.
    let point: NSPoint = unsafe { msg_send![class!(NSEvent), mouseLocation] };
    screens
        .iter()
        .find(|(frame, _)| rect_contains(frame, point.x, point.y))?;
    Some((point.x, flip_y(primary_h, point.y)))
}

/// `visibleFrame` as `(x, y, w, h)` of the display containing the given
/// top-left-origin point, converted to top-left origin as well. The visible
/// frame excludes the menu bar and the Dock, so a window clamped into it stays
/// fully reachable. `None` when no display contains the point.
pub fn screen_visible_frame_at(x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
    let screens = screen_frames();
    let primary_h = primary_height(&screens)?;
    let y_bottom_left = flip_y(primary_h, y);
    let (_, visible) = screens
        .iter()
        .find(|(frame, _)| rect_contains(frame, x, y_bottom_left))?;
    Some(rect_to_top_left(
        primary_h,
        (
            visible.origin.x,
            visible.origin.y,
            visible.size.width,
            visible.size.height,
        ),
    ))
}

fn post_key(keycode: u16, flags: CGEventFlags, key_down: bool) -> Result<()> {
    // CombinedSessionState + Session tap matches working macOS clipboard tools (e.g. Maccy).
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| anyhow!("CGEventSource::new failed"))?;
    let event = CGEvent::new_keyboard_event(source, keycode, key_down)
        .map_err(|_| anyhow!("CGEvent::new_keyboard_event failed"))?;
    event.set_flags(flags);
    event.post(CGEventTapLocation::Session);
    Ok(())
}

fn cmd_keystroke(keycode: u16) -> Result<()> {
    let flags = CGEventFlags::CGEventFlagCommand;
    post_key(keycode, flags, true)?;
    thread::sleep(Duration::from_millis(20));
    post_key(keycode, flags, false)?;
    thread::sleep(Duration::from_millis(60));
    Ok(())
}

fn clipboard_copy() -> Result<()> {
    cmd_keystroke(KeyCode::ANSI_C)
}

fn clipboard_paste() -> Result<()> {
    cmd_keystroke(KeyCode::ANSI_V)
}

/// Whole-pasteboard snapshot/restore over `NSPasteboard`, so the clipboard
/// fallbacks preserve images and rich content, not just the text `arboard` sees.
mod pasteboard {
    use anyhow::{bail, Result};
    use cocoa::base::id;
    use objc::rc::autoreleasepool;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::c_void;
    use std::sync::{Mutex, MutexGuard};

    /// Serializes every in-process access to the general pasteboard.
    ///
    /// `NSPasteboard`'s type cache is not thread-safe: two threads calling
    /// `types` / `dataForType:` on `generalPasteboard` at once crash inside
    /// `-[NSPasteboard _updateTypeCacheIfNeeded]` (observed under `cargo test`).
    /// The delayed-restore thread can overlap a later selection read, so every
    /// entry point below and the arboard helpers in the parent module take this.
    static PASTEBOARD_LOCK: Mutex<()> = Mutex::new(());

    pub(super) fn lock() -> MutexGuard<'static, ()> {
        PASTEBOARD_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Plain Rust copy of every item on the general pasteboard: for each item,
    /// the `(type, bytes)` pairs it carried. Owns no Objective-C objects, so it
    /// is `Send` and can move into the delayed-restore thread.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct PasteboardSnapshot {
        /// `NSPasteboard.changeCount` at the time of the snapshot.
        pub(super) change_count: i64,
        pub(super) items: Vec<Vec<(String, Vec<u8>)>>,
    }

    impl PasteboardSnapshot {
        /// Bytes stored for `ty` on the first item that carries it.
        #[cfg(test)]
        pub(super) fn bytes_for(&self, ty: &str) -> Option<&[u8]> {
            self.items
                .iter()
                .flatten()
                .find(|(t, _)| t == ty)
                .map(|(_, b)| b.as_slice())
        }
    }

    /// `[NSPasteboard generalPasteboard]`: a process-wide singleton owned by
    /// AppKit; never released by us.
    unsafe fn general_pasteboard() -> Result<id> {
        let pb: id = msg_send![class!(NSPasteboard), generalPasteboard];
        if pb.is_null() {
            bail!("NSPasteboard.generalPasteboard returned nil");
        }
        Ok(pb)
    }

    /// Copy an `NSString` into an owned Rust `String` (None for nil / non-UTF-8).
    unsafe fn nsstring_to_string(s: id) -> Option<String> {
        if s.is_null() {
            return None;
        }
        // `UTF8String` is valid for the lifetime of the (autoreleased) NSString; we copy it out.
        let utf8: *const std::os::raw::c_char = msg_send![s, UTF8String];
        if utf8.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(utf8)
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// The UTI every plain-text paste consumer reads (`NSPasteboardTypeString`).
    pub(super) const TEXT_TYPE: &str = "public.utf8-plain-text";

    /// A +1 `NSString` copied from `s`, or nil.
    unsafe fn nsstring_new(s: &str) -> id {
        let obj: id = msg_send![class!(NSString), alloc];
        // initWithBytes:length:encoding: copies the UTF-8 (NSUTF8StringEncoding = 4).
        msg_send![obj,
            initWithBytes: s.as_ptr() as *const c_void
            length: s.len()
            encoding: 4u64]
    }

    /// Read the plain text while the caller owns `PASTEBOARD_LOCK`.
    unsafe fn pasteboard_text_locked(pb: id) -> Option<String> {
        let ty = nsstring_new(TEXT_TYPE);
        if ty.is_null() {
            return None;
        }
        let s: id = msg_send![pb, stringForType: ty];
        let out = nsstring_to_string(s);
        let _: () = msg_send![ty, release];
        out
    }

    /// Replace the pasteboard with `text` and return the `changeCount` that
    /// *this* write produced.
    ///
    /// The count has to come out of the write itself. Writing and then reading
    /// `changeCount` as two calls leaves a window in which another process can
    /// write: the number read back would then be that stranger's, the delayed
    /// restore would see it unchanged, and it would overwrite their content
    /// while believing it was still holding Selara's own paste text.
    /// `-[NSPasteboard clearContents]` closes that window — it bumps the count
    /// and returns the new value in a single pasteboard-server round trip, and
    /// the `setString:forType:` that follows writes into the session that call
    /// opened without bumping it again.
    pub(super) fn write_text(text: &str) -> Result<i64> {
        let _guard = lock();
        autoreleasepool(|| {
            // SAFETY: generalPasteboard is a live singleton; both NSStrings are +1
            // and released before returning, and setString: copies the contents.
            unsafe {
                let pb = general_pasteboard()?;
                let ours: i64 = msg_send![pb, clearContents];
                let value = nsstring_new(text);
                if value.is_null() {
                    bail!("NSString for the paste text returned nil");
                }
                let ty = nsstring_new(TEXT_TYPE);
                if ty.is_null() {
                    let _: () = msg_send![value, release];
                    bail!("NSString for the pasteboard type returned nil");
                }
                let ok: bool = msg_send![pb, setString: value forType: ty];
                let _: () = msg_send![value, release];
                let _: () = msg_send![ty, release];
                if !ok {
                    bail!("NSPasteboard setString:forType: returned NO");
                }
                Ok(ours)
            }
        })
    }

    /// The plain text currently on the general pasteboard, if any.
    #[cfg(test)]
    pub(super) fn pasteboard_text() -> Option<String> {
        let _guard = lock();
        autoreleasepool(|| {
            // SAFETY: `pasteboard_text_locked` only uses the live pasteboard and
            // copies the autoreleased NSString into an owned Rust String.
            unsafe {
                let pb = general_pasteboard().ok()?;
                pasteboard_text_locked(pb)
            }
        })
    }

    /// Return the text and change count from one locked pasteboard observation.
    /// This is used after a synthetic copy so a user's intervening clipboard
    /// write cannot be mistaken for the copied selection.
    pub(super) fn text_and_change_count() -> (i64, Option<String>) {
        let _guard = lock();
        autoreleasepool(|| unsafe {
            let Ok(pb) = general_pasteboard() else {
                return (-1, None);
            };
            let count: i64 = msg_send![pb, changeCount];
            (count, pasteboard_text_locked(pb))
        })
    }

    /// Check ownership immediately before a synthetic paste or restore.
    pub(super) fn owns_text(change_count: i64, expected: &str) -> bool {
        let _guard = lock();
        autoreleasepool(|| unsafe {
            let Ok(pb) = general_pasteboard() else {
                return false;
            };
            let now: i64 = msg_send![pb, changeCount];
            now == change_count && pasteboard_text_locked(pb).as_deref() == Some(expected)
        })
    }

    /// Current `NSPasteboard.changeCount`; bumps on every write by any process.
    #[cfg(test)]
    pub(super) fn pasteboard_change_count() -> i64 {
        let _guard = lock();
        // SAFETY: generalPasteboard is a live singleton; changeCount returns a plain NSInteger.
        unsafe {
            match general_pasteboard() {
                Ok(pb) => msg_send![pb, changeCount],
                Err(_) => -1,
            }
        }
    }

    /// Copy every item and type on the general pasteboard into Rust memory.
    pub(super) fn snapshot_pasteboard() -> Result<PasteboardSnapshot> {
        let _guard = lock();
        autoreleasepool(|| {
            // SAFETY: all objects below are owned by the pasteboard or the surrounding
            // autorelease pool; every buffer is copied into a Vec before the pool drains.
            unsafe {
                let pb = general_pasteboard()?;
                let change_count: i64 = msg_send![pb, changeCount];
                let items: id = msg_send![pb, pasteboardItems];
                let mut out = Vec::new();
                if items.is_null() {
                    return Ok(PasteboardSnapshot {
                        change_count,
                        items: out,
                    });
                }
                let item_count: usize = msg_send![items, count];
                for i in 0..item_count {
                    let item: id = msg_send![items, objectAtIndex: i];
                    if item.is_null() {
                        continue;
                    }
                    let types: id = msg_send![item, types];
                    let mut entry = Vec::new();
                    if !types.is_null() {
                        let type_count: usize = msg_send![types, count];
                        for j in 0..type_count {
                            let ty: id = msg_send![types, objectAtIndex: j];
                            let Some(name) = nsstring_to_string(ty) else {
                                continue;
                            };
                            let data: id = msg_send![item, dataForType: ty];
                            if data.is_null() {
                                continue;
                            }
                            let len: usize = msg_send![data, length];
                            let bytes: *const u8 = msg_send![data, bytes];
                            let buf = if len == 0 || bytes.is_null() {
                                Vec::new()
                            } else {
                                // `bytes` is valid for `len` bytes while `data` is alive (the pool).
                                std::slice::from_raw_parts(bytes, len).to_vec()
                            };
                            entry.push((name, buf));
                        }
                    }
                    out.push(entry);
                }
                Ok(PasteboardSnapshot {
                    change_count,
                    items: out,
                })
            }
        })
    }

    /// Clear the general pasteboard and write the snapshot back, one
    /// `NSPasteboardItem` per saved item. An empty snapshot leaves it cleared.
    unsafe fn restore_pasteboard_locked(pb: id, snap: &PasteboardSnapshot) -> Result<()> {
        let _: i64 = msg_send![pb, clearContents];
        if snap.items.is_empty() {
            return Ok(());
        }
        let array: id = msg_send![class!(NSMutableArray), arrayWithCapacity: snap.items.len()];
        for item in &snap.items {
            // `new` returns +1; the array retains it in addObject:, so we release ours.
            let pb_item: id = msg_send![class!(NSPasteboardItem), new];
            if pb_item.is_null() {
                bail!("NSPasteboardItem new returned nil");
            }
            for (ty, bytes) in item {
                let ty_str: id = msg_send![class!(NSString), alloc];
                // initWithBytes:length:encoding: copies the UTF-8 (NSUTF8StringEncoding = 4).
                let ty_str: id = msg_send![ty_str,
                    initWithBytes: ty.as_ptr() as *const c_void
                    length: ty.len()
                    encoding: 4u64];
                if ty_str.is_null() {
                    continue;
                }
                let data: id = msg_send![class!(NSData),
                    dataWithBytes: bytes.as_ptr() as *const c_void
                    length: bytes.len()];
                let _: bool = msg_send![pb_item, setData: data forType: ty_str];
                // setData:forType: copies the type key; drop our +1 on the NSString.
                let _: () = msg_send![ty_str, release];
            }
            let _: () = msg_send![array, addObject: pb_item];
            let _: () = msg_send![pb_item, release];
        }
        let ok: bool = msg_send![pb, writeObjects: array];
        if !ok {
            bail!("NSPasteboard writeObjects: returned NO");
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn restore_pasteboard(snap: &PasteboardSnapshot) -> Result<()> {
        let _guard = lock();
        autoreleasepool(|| unsafe {
            let pb = general_pasteboard()?;
            restore_pasteboard_locked(pb, snap)
        })
    }

    /// Restore only while the pasteboard is still ours. The ownership check and
    /// restore share one lock, closing the race between two separate calls.
    pub(super) fn restore_if_owned(
        snap: &PasteboardSnapshot,
        expected_change_count: i64,
        expected_text: Option<&str>,
    ) -> Result<bool> {
        let _guard = lock();
        autoreleasepool(|| unsafe {
            let pb = general_pasteboard()?;
            let now: i64 = msg_send![pb, changeCount];
            if now != expected_change_count {
                return Ok(false);
            }
            if let Some(expected) = expected_text {
                if pasteboard_text_locked(pb).as_deref() != Some(expected) {
                    return Ok(false);
                }
            }
            restore_pasteboard_locked(pb, snap)?;
            Ok(true)
        })
    }
}

use pasteboard::{restore_if_owned, snapshot_pasteboard};

/// An Accessibility element identity retained independently of the AX wrapper.
///
/// `AXUIElement` is a Core Foundation wrapper and is intentionally kept out of
/// [`SelectionSnapshot`], which crosses async task boundaries.  We retain the
/// underlying object while a captured selection is live and compare it to the
/// target element immediately before a paste.
#[derive(Debug)]
struct CapturedElement {
    raw: usize,
}

impl CapturedElement {
    fn new(element: &AXUIElement) -> Self {
        let raw = element.as_CFTypeRef();
        // SAFETY: `raw` is a live AXUIElement retained for this identity's
        // lifetime.  The matching CFRelease is in Drop below.
        unsafe { core_foundation::base::CFRetain(raw) };
        Self { raw: raw as usize }
    }

    fn matches(&self, element: &AXUIElement) -> bool {
        if self.raw == 0 {
            return false;
        }
        // SAFETY: `self.raw` is retained by this value and `element` is live
        // for the duration of the comparison.
        unsafe {
            core_foundation::base::CFEqual(self.raw as CFTypeRef, element.as_CFTypeRef()) != 0
        }
    }
}

impl Clone for CapturedElement {
    fn clone(&self) -> Self {
        if self.raw != 0 {
            // SAFETY: `raw` remains retained by the source identity while the
            // clone acquires its own retain.
            unsafe { core_foundation::base::CFRetain(self.raw as CFTypeRef) };
        }
        Self { raw: self.raw }
    }
}

impl Drop for CapturedElement {
    fn drop(&mut self) {
        if self.raw != 0 {
            // SAFETY: this is the retain acquired in `CapturedElement::new`.
            unsafe { core_foundation::base::CFRelease(self.raw as CFTypeRef) };
        }
    }
}

#[derive(Debug)]
struct CapturedSelection {
    pid: Option<i32>,
    element: CapturedElement,
    window: CapturedElement,
    original: String,
    range: Option<(i64, i64)>,
}

#[derive(Debug, Clone)]
struct CapturedTarget {
    element: CapturedElement,
    window: CapturedElement,
}

/// Owns a pasteboard write until the target has consumed and verified it. If
/// pre-paste validation fails, Drop restores the old multi-item pasteboard only
/// when no newer writer has taken ownership.
struct PasteboardOwnership {
    snapshot: Option<pasteboard::PasteboardSnapshot>,
    change_count: i64,
    text: String,
}

/// Result of a fresh Cmd+C capture. The previous pasteboard is restored only
/// after the copied text is verified against the captured AX control. If that
/// verification fails, leaving the fresh copy in place is safer than guessing
/// that a concurrent user copy belonged to Selara.
struct ClipboardCapture {
    snapshot: pasteboard::PasteboardSnapshot,
    change_count: i64,
    text: String,
}

impl ClipboardCapture {
    fn restore(self) {
        match restore_if_owned(&self.snapshot, self.change_count, Some(self.text.as_str())) {
            Ok(true) => debug!("restored pasteboard after selection capture"),
            Ok(false) => debug!("newer clipboard contents won; leaving pasteboard alone"),
            Err(e) => warn!("restore pasteboard after selection capture failed ({e})"),
        }
    }
}

impl PasteboardOwnership {
    fn new(snapshot: pasteboard::PasteboardSnapshot, change_count: i64, text: &str) -> Self {
        Self {
            snapshot: Some(snapshot),
            change_count,
            text: text.to_string(),
        }
    }

    fn take(&mut self) -> Option<(pasteboard::PasteboardSnapshot, i64, String)> {
        self.snapshot
            .take()
            .map(|snapshot| (snapshot, self.change_count, self.text.clone()))
    }
}

impl Drop for PasteboardOwnership {
    fn drop(&mut self) {
        let Some(snapshot) = self.snapshot.take() else {
            return;
        };
        match restore_if_owned(&snapshot, self.change_count, Some(self.text.as_str())) {
            Ok(true) => debug!("restored pasteboard after refusing an unsafe paste"),
            Ok(false) => debug!("newer clipboard contents won; leaving pasteboard alone"),
            Err(e) => warn!("restore pasteboard after refusing paste failed ({e})"),
        }
    }
}

fn replacement_verified(
    target_pid: i32,
    identity: &CapturedTarget,
    text: &str,
    range: Option<(i64, i64)>,
    before_value: Option<&str>,
) -> bool {
    let output_units = text.encode_utf16().count() as i64;
    let expected_after = before_value
        .and_then(|before| range.and_then(|range| replace_utf16_range(before, range, text)));
    let require_full_value = before_value.is_some() && range.is_some();
    // Some editors leave the inserted text selected, while others collapse to
    // a caret. Poll briefly for both representations without changing the
    // user's current selection.
    for _ in 0..25 {
        if frontmost_pid() != Some(target_pid) {
            return false;
        }
        let live = match focused_element_for_pid(target_pid) {
            Ok(live) if identity.element.matches(&live) => live,
            _ => return false,
        };
        let _live_window = match focused_window_for_pid(target_pid) {
            Ok(window) if identity.window.matches(&window) => window,
            _ => return false,
        };
        let selected = read_ax_selected_text(&live).ok();
        if selected.as_deref() == Some(text) {
            if require_full_value {
                if expected_after.as_deref() == read_ax_value_text(&live).ok().as_deref() {
                    return true;
                }
            } else if let Some((location, _)) = range {
                if read_ax_selected_range(&live) == Some((location, output_units)) {
                    return true;
                }
            } else {
                return true;
            }
        }
        if let Some(expected) = expected_after.as_deref() {
            if read_ax_value_text(&live).ok().as_deref() == Some(expected) {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn clip_get(clip: &Mutex<Clipboard>) -> Result<Option<String>> {
    // arboard talks to the same NSPasteboard; keep it off the delayed-restore thread's toes.
    let _pb = pasteboard::lock();
    let mut c = clip.lock().unwrap_or_else(|e| e.into_inner());
    match c.get_text() {
        Ok(t) => Ok(Some(t)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

// There is no `clip_set`: writing the paste text goes through
// `pasteboard::write_text`, which returns the changeCount its own write
// produced. arboard cannot report that, and reading the count back afterwards
// is exactly the race the delayed restore has to be immune to.

pub struct MacosSelection {
    clipboard: Mutex<Clipboard>,
    captured: Mutex<Option<CapturedSelection>>,
}

impl MacosSelection {
    pub fn new() -> Result<Self> {
        Ok(Self {
            clipboard: Mutex::new(Clipboard::new().context("open clipboard")?),
            captured: Mutex::new(None),
        })
    }

    /// Text sitting on the pasteboard right now, or `None` when it is empty
    /// or holds nothing but whitespace.
    ///
    /// Reads through the same process-wide pasteboard lock as the copy/paste
    /// fallbacks, so a delayed restore cannot land in the middle of the read.
    /// Nothing is written and no key event is synthesized.
    pub fn clipboard_text(&self) -> Result<Option<String>> {
        Ok(clip_get(&self.clipboard)?.filter(|t| !t.trim().is_empty()))
    }

    fn remember_selection(
        &self,
        pid: Option<i32>,
        element: &AXUIElement,
        original: &str,
        range: Option<(i64, i64)>,
    ) -> Result<()> {
        let pid = pid.context("selected app has no process identifier")?;
        let window = focused_window_for_pid(pid).context("selected app has no focused window")?;
        let captured = CapturedSelection {
            pid: Some(pid),
            element: CapturedElement::new(element),
            window: CapturedElement::new(&window),
            original: original.to_string(),
            range,
        };
        *self.captured.lock().unwrap_or_else(|e| e.into_inner()) = Some(captured);
        Ok(())
    }

    fn read_via_clipboard_fallback(&self) -> Result<Option<ClipboardCapture>> {
        let snap = snapshot_pasteboard().context("snapshot pasteboard before ⌘C")?;
        clipboard_copy()?;
        thread::sleep(Duration::from_millis(30));
        let (first_count, _) = pasteboard::text_and_change_count();
        thread::sleep(Duration::from_millis(50));
        // Observe the copied text and its ownership token together. A later
        // restore is allowed only while both still describe our copy.
        let (ours, copied) = pasteboard::text_and_change_count();
        if ours == snap.change_count || ours != first_count {
            return Ok(None);
        }
        let copied = copied.filter(|t| !t.is_empty());
        Ok(copied.map(|text| ClipboardCapture {
            snapshot: snap,
            change_count: ours,
            text,
        }))
    }

    fn replace_via_clipboard_fallback(
        &self,
        target_pid: i32,
        identity: &CapturedTarget,
        original: &str,
        text: &str,
        range: Option<(i64, i64)>,
    ) -> Result<()> {
        let snap = snapshot_pasteboard().context("snapshot pasteboard before ⌘V")?;
        // Write and read the guard in one call: `clearContents` returns the
        // changeCount it produced, so `ours` is always Selara's own write and
        // never an intervening one from another process.
        let ours =
            pasteboard::write_text(text).context("write the paste text to the pasteboard")?;
        let mut ownership = PasteboardOwnership::new(snap, ours, text);
        // Let the pasteboard settle before synthesizing ⌘V.
        thread::sleep(Duration::from_millis(80));
        if frontmost_pid() != Some(target_pid) {
            bail!("the selected app is no longer frontmost; refusing to paste");
        }
        let current = focused_element_for_pid(target_pid)
            .context("the selected control is no longer available")?;
        let window = focused_window_for_pid(target_pid)
            .context("the selected window is no longer available")?;
        if !identity.element.matches(&current) || !identity.window.matches(&window) {
            bail!("the selected control changed before paste");
        }
        self.ensure_original_selected(&current, original, range)?;
        let before_value = read_ax_value_text(&current).ok();
        // This check is intentionally adjacent to Cmd+V. AX validation above
        // can block long enough for a newer user clipboard write to arrive.
        if frontmost_pid() != Some(target_pid) {
            bail!("the selected app is no longer frontmost; refusing to paste");
        }
        if !pasteboard::owns_text(ours, text) {
            bail!("clipboard changed before paste; refusing to paste into the target");
        }
        clipboard_paste()?;
        // Keep ownership until verification. On uncertain output we leave the
        // pasteboard untouched rather than racing a slow consumer or restoring
        // stale content after a failed paste.
        let Some((snap, ours, written)) = ownership.take() else {
            bail!("pasteboard ownership was lost before paste verification");
        };

        // A paste that cannot be observed is unsafe to report as a successful
        // replacement. The expected output range is read from AXValue so this
        // check never steals a newer user caret by selecting the output.
        let verified =
            replacement_verified(target_pid, identity, text, range, before_value.as_deref());
        if !verified {
            bail!(
                "paste completed but the target app did not expose the expected replacement; \
                 refusing to repeat it"
            );
        }
        match restore_if_owned(&snap, ours, Some(written.as_str())) {
            Ok(true) => debug!(
                items = snap.items.len(),
                "restored pasteboard snapshot after paste"
            ),
            Ok(false) => debug!("pasteboard changed since our paste; leaving it alone"),
            Err(e) => warn!("restore pasteboard after ⌘V failed ({e})"),
        }
        Ok(())
    }

    fn captured_target(
        &self,
        pid: Option<i32>,
        original: &str,
        range: Option<(i64, i64)>,
    ) -> Result<(i32, CapturedTarget)> {
        let captured = self.captured.lock().unwrap_or_else(|e| e.into_inner());
        let Some(captured) = captured.as_ref() else {
            bail!("no selection was captured for this command");
        };
        let target_pid = pid
            .or(captured.pid)
            .context("captured selection has no process")?;
        if captured.pid != Some(target_pid) {
            bail!("the selected app changed before replacement");
        }
        if captured.original != original {
            bail!("the selected text changed before replacement");
        }
        if captured.range != range {
            bail!("the selected range changed before replacement");
        }
        Ok((
            target_pid,
            CapturedTarget {
                element: captured.element.clone(),
                window: captured.window.clone(),
            },
        ))
    }

    /// Validate the captured target before spending provider time. This only
    /// observes the target and never replaces or recaptures its identity.
    pub fn validate_captured_selection(
        &self,
        pid: Option<i32>,
        original: &str,
        range: Option<(i64, i64)>,
    ) -> Result<()> {
        if !accessibility_trusted() {
            bail!(
                "Accessibility permission missing. Enable Selara (or the Terminal/binary \
                 you launched) under System Settings → Privacy & Security → Accessibility."
            );
        }
        let (target_pid, identity) = self.captured_target(pid, original, range)?;
        if frontmost_pid() != Some(target_pid) {
            bail!("the selected app is no longer frontmost");
        }
        let element = focused_element_for_pid(target_pid)
            .context("the selected control is no longer available")?;
        let window = focused_window_for_pid(target_pid)
            .context("the selected window is no longer available")?;
        if !identity.element.matches(&element) || !identity.window.matches(&window) {
            bail!("the selected control changed before replacement");
        }
        self.ensure_original_selected(&element, original, range)
    }

    fn ensure_original_selected(
        &self,
        element: &AXUIElement,
        original: &str,
        range: Option<(i64, i64)>,
    ) -> Result<()> {
        if let Some(expected_range) = range {
            let current_range = read_ax_selected_range(element)
                .context("could not verify the captured selection range")?;
            if current_range != expected_range {
                bail!("the selected range changed before replacement");
            }
        }
        let current =
            read_ax_selected_text(element).context("could not verify the captured selection")?;
        if current != original {
            bail!("the selected text changed before replacement");
        }
        Ok(())
    }

    /// Replace selection in a previously focused app with the target app's
    /// normal Cmd+V operation. Call after hiding our UI and reactivating the
    /// captured target.
    pub fn replace_in_app(
        &self,
        pid: Option<i32>,
        text: &str,
        original: &str,
        range: Option<(i64, i64)>,
    ) -> Result<()> {
        if original.is_empty() {
            bail!("cannot replace an empty selection");
        }
        if !accessibility_trusted() {
            bail!(
                "Accessibility permission missing. Enable Selara (or the Terminal/binary \
                 you launched) under System Settings → Privacy & Security → Accessibility."
            );
        }
        let (target_pid, identity) = self.captured_target(pid, original, range)?;
        if frontmost_pid() != Some(target_pid) {
            bail!("the selected app is no longer frontmost");
        }
        let element = focused_element_for_pid(target_pid)
            .context("the selected control is no longer available")?;
        let window = focused_window_for_pid(target_pid)
            .context("the selected window is no longer available")?;
        if !identity.element.matches(&element) || !identity.window.matches(&window) {
            bail!("the selected control changed before replacement");
        }
        self.ensure_original_selected(&element, original, range)?;
        if text == original {
            self.captured
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            return Ok(());
        }
        self.replace_via_clipboard_fallback(target_pid, &identity, original, text, range)
            .context("clipboard replace fallback")?;
        self.captured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        debug!(len = text.len(), "replaced selection via Cmd+V");
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
                "Accessibility permission missing. Enable Selara (or the Terminal/binary \
                 you launched) under System Settings → Privacy & Security → Accessibility."
            );
        }

        let app_name = frontmost_app_name();
        let bundle_id = frontmost_bundle_id();
        let pid = frontmost_pid();
        // A failed/empty capture must never leave an earlier command's target
        // available for a later replacement.
        self.captured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();

        match focused_element() {
            Ok(el) => {
                let range = read_ax_selected_range(&el);
                match read_ax_selected_text(&el) {
                    Ok(text) if !text.is_empty() && range.is_some() => {
                        debug!(len = text.len(), "read selection via AX");
                        self.remember_selection(pid, &el, &text, range)?;
                        return Ok(Some(SelectionSnapshot {
                            text,
                            app_name,
                            bundle_id,
                            range,
                        }));
                    }
                    Ok(_) => debug!("AX selected text empty; trying clipboard fallback"),
                    Err(e) => debug!("AX read failed ({e}); trying clipboard fallback"),
                }

                let capture = self
                    .read_via_clipboard_fallback()
                    .context("clipboard selection fallback")?;
                if let Some(capture) = capture {
                    let text = capture.text.clone();
                    // The copy was fresh, and the captured control/range is the
                    // identity used for revalidation. If the control itself
                    // changed during Cmd+C, refuse to treat its text as a
                    // selection.
                    if range.is_none()
                        || read_ax_selected_text(&el).ok().as_deref() != Some(text.as_str())
                    {
                        return Ok(None);
                    }
                    let same_target = frontmost_pid() == pid
                        && focused_element()
                            .ok()
                            .map(|current| CapturedElement::new(&el).matches(&current))
                            .unwrap_or(false);
                    if !same_target {
                        return Ok(None);
                    }
                    capture.restore();
                    self.remember_selection(pid, &el, &text, range)?;
                    return Ok(Some(SelectionSnapshot {
                        text,
                        app_name,
                        bundle_id,
                        range,
                    }));
                }
                return Ok(None);
            }
            Err(e) => debug!("focused element unavailable ({e}); trying clipboard fallback"),
        }
        // Clipboard contents without a focused AX control are not a selection.
        Ok(None)
    }

    async fn replace_selection(&self, _text: &str) -> Result<()> {
        let (pid, original, range) = {
            let captured = self.captured.lock().unwrap_or_else(|e| e.into_inner());
            let Some(captured) = captured.as_ref() else {
                bail!("replace_selection requires a captured non-empty selection");
            };
            (captured.pid, captured.original.clone(), captured.range)
        };
        self.replace_in_app(pid, _text, &original, range)
    }
}

#[cfg(test)]
mod tests {
    use super::pasteboard::{
        pasteboard_change_count, pasteboard_text, restore_pasteboard, snapshot_pasteboard,
        write_text, PasteboardSnapshot,
    };
    use arboard::Clipboard;
    use std::sync::{Mutex, MutexGuard};

    #[test]
    fn flip_y_is_its_own_inverse() {
        // Primary display 1080 pt tall: 20 pt above the bottom edge is 1060 pt
        // below the top edge, and flipping again gets the original back.
        assert_eq!(super::flip_y(1080.0, 20.0), 1060.0);
        assert_eq!(super::flip_y(1080.0, super::flip_y(1080.0, 20.0)), 20.0);
    }

    #[test]
    fn visible_frame_flips_to_top_left_on_the_primary_display() {
        // Menu bar 25 pt, Dock 70 pt on a 1920×1080 display: AppKit reports the
        // visible frame as origin (0, 70) with height 985.
        assert_eq!(
            super::rect_to_top_left(1080.0, (0.0, 70.0, 1920.0, 985.0)),
            (0.0, 25.0, 1920.0, 985.0)
        );
    }

    #[test]
    fn visible_frame_on_a_secondary_display_uses_the_primary_height() {
        // A 1440-pt-tall display to the right whose top is 200 pt above the
        // primary's top edge: AppKit frame origin y is 1080 + 200 - 1440 = -160.
        // Its top edge in top-left coordinates is therefore -200.
        assert_eq!(
            super::rect_to_top_left(1080.0, (1920.0, -160.0, 2560.0, 1440.0)),
            (1920.0, -200.0, 2560.0, 1440.0)
        );
    }

    /// Both tests own the one general pasteboard for their whole body; without
    /// this, one test's teardown restore lands between another test's steps.
    static TEST_PASTEBOARD: Mutex<()> = Mutex::new(());

    fn exclusive() -> MutexGuard<'static, ()> {
        TEST_PASTEBOARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    const TEXT_TYPE: &str = "public.utf8-plain-text";
    const CUSTOM_TYPE: &str = "dev.snowops.selara.test";
    const CUSTOM_BYTES: &[u8] = b"\x00\x01\x02";

    /// Puts the developer's pasteboard back even if an assertion panics.
    struct RestoreOnDrop(PasteboardSnapshot);

    impl Drop for RestoreOnDrop {
        fn drop(&mut self) {
            if let Err(e) = restore_pasteboard(&self.0) {
                eprintln!("could not restore the original pasteboard: {e}");
            }
        }
    }

    #[test]
    fn snapshot_round_trips_custom_types_through_restore() {
        let _exclusive = exclusive();
        let _guard = RestoreOnDrop(snapshot_pasteboard().expect("initial snapshot"));

        let synthetic = PasteboardSnapshot {
            change_count: 0,
            items: vec![vec![
                (TEXT_TYPE.to_string(), b"before".to_vec()),
                (CUSTOM_TYPE.to_string(), CUSTOM_BYTES.to_vec()),
            ]],
        };
        let before_write = pasteboard_change_count();
        restore_pasteboard(&synthetic).expect("write synthetic snapshot");
        assert_ne!(
            pasteboard_change_count(),
            before_write,
            "writing bumps changeCount"
        );

        let first = snapshot_pasteboard().expect("snapshot after synthetic write");
        assert_eq!(first.items.len(), 1, "one pasteboard item");
        assert_eq!(first.bytes_for(TEXT_TYPE), Some(&b"before"[..]));
        assert_eq!(first.bytes_for(CUSTOM_TYPE), Some(CUSTOM_BYTES));
        assert_eq!(first.change_count, pasteboard_change_count());

        // Something else takes over the pasteboard (what our paste path does via arboard).
        Clipboard::new()
            .expect("open clipboard")
            .set_text("something else".to_string())
            .expect("set text");
        let clobbered = snapshot_pasteboard().expect("snapshot after arboard write");
        assert_eq!(clobbered.bytes_for(TEXT_TYPE), Some(&b"something else"[..]));
        assert_eq!(
            clobbered.bytes_for(CUSTOM_TYPE),
            None,
            "custom type is gone"
        );
        assert_ne!(clobbered.change_count, first.change_count);

        // Restoring the snapshot brings the custom payload back byte for byte.
        restore_pasteboard(&first).expect("restore snapshot");
        let second = snapshot_pasteboard().expect("snapshot after restore");
        assert_eq!(second.bytes_for(CUSTOM_TYPE), Some(CUSTOM_BYTES));
        assert_eq!(second.bytes_for(TEXT_TYPE), Some(&b"before"[..]));
        assert_eq!(second.items, first.items);

        // arboard still reads the plain text out of a restored snapshot.
        let text = Clipboard::new()
            .expect("open clipboard")
            .get_text()
            .expect("get text");
        assert_eq!(text, "before");
    }

    #[test]
    fn restoring_an_empty_snapshot_clears_the_pasteboard() {
        let _exclusive = exclusive();
        let _guard = RestoreOnDrop(snapshot_pasteboard().expect("initial snapshot"));

        Clipboard::new()
            .expect("open clipboard")
            .set_text("to be cleared".to_string())
            .expect("set text");
        let empty = PasteboardSnapshot {
            change_count: 0,
            items: Vec::new(),
        };
        restore_pasteboard(&empty).expect("restore empty snapshot");
        let after = snapshot_pasteboard().expect("snapshot after clear");
        assert!(after.items.is_empty(), "pasteboard has no items: {after:?}");
    }

    /// The guard the delayed restore relies on: the changeCount it compares
    /// against has to be the one our own write produced, not one read back
    /// afterwards, which another process could have bumped in between.
    #[test]
    fn write_text_returns_the_change_count_of_its_own_write() {
        let _exclusive = exclusive();
        let _guard = RestoreOnDrop(snapshot_pasteboard().expect("initial snapshot"));

        let before = pasteboard_change_count();
        let ours = write_text("selara paste text").expect("write paste text");

        assert!(
            ours > before,
            "the write bumped the count: {before} -> {ours}"
        );
        assert_eq!(
            pasteboard_change_count(),
            ours,
            "nothing bumps the count between the write and a later read"
        );
        assert_eq!(pasteboard_text().as_deref(), Some("selara paste text"));

        // A later write by anyone else moves the count away, which is what makes
        // the delayed restore back off instead of clobbering their content.
        let theirs = write_text("someone else's copy").expect("second write");
        assert_ne!(theirs, ours);
        assert_eq!(pasteboard_text().as_deref(), Some("someone else's copy"));
    }

    #[test]
    fn guarded_restore_preserves_a_newer_copy() {
        let _exclusive = exclusive();
        let original = snapshot_pasteboard().expect("initial snapshot");
        let _guard = RestoreOnDrop(original);
        let before = snapshot_pasteboard().expect("saved clipboard");
        let ours = write_text("generated replacement").expect("write output");
        assert!(super::pasteboard::owns_text(ours, "generated replacement"));

        Clipboard::new()
            .expect("open clipboard")
            .set_text("newer user copy".to_string())
            .expect("copy while replacement is pending");
        assert!(!super::pasteboard::owns_text(ours, "generated replacement"));
        assert!(
            !super::pasteboard::restore_if_owned(&before, ours, Some("generated replacement"))
                .expect("attempt guarded restore")
        );
        assert_eq!(pasteboard_text().as_deref(), Some("newer user copy"));
    }

    #[test]
    fn guarded_restore_checks_text_and_restores_all_formats() {
        let _exclusive = exclusive();
        let _guard = RestoreOnDrop(snapshot_pasteboard().expect("initial snapshot"));
        let before = PasteboardSnapshot {
            change_count: 0,
            items: vec![vec![
                (TEXT_TYPE.to_string(), b"original copy".to_vec()),
                (CUSTOM_TYPE.to_string(), CUSTOM_BYTES.to_vec()),
            ]],
        };
        let ours = write_text("generated replacement").expect("write output");
        assert!(
            !super::pasteboard::restore_if_owned(&before, ours, Some("wrong text"))
                .expect("reject mismatched output")
        );
        assert_eq!(pasteboard_text().as_deref(), Some("generated replacement"));
        assert!(
            super::pasteboard::restore_if_owned(&before, ours, Some("generated replacement"))
                .expect("restore owned clipboard")
        );
        assert_eq!(
            snapshot_pasteboard().expect("restored formats").items,
            before.items
        );
    }

    #[test]
    fn replacement_ranges_use_utf16_code_units() {
        assert_eq!(
            super::replace_utf16_range("a🙂b", (1, 2), "x").as_deref(),
            Some("axb")
        );
        assert_eq!(
            super::replace_utf16_range("foobar", (0, 6), "foo").as_deref(),
            Some("foo")
        );
        assert!(super::replace_utf16_range("abc", (2, 2), "x").is_none());
    }
}
