//! Selection read/replace via Accessibility, with Cmd+C / Cmd+V clipboard fallback.
//!
//! Tradeoff: AX `AXSelectedText` set is preferred when the focused element supports
//! it. Many apps ignore setValue; the fallback snapshots the whole pasteboard
//! (every item and every type, not just text), pastes the result with Cmd+V,
//! then restores the snapshot after a short delay.
//!
//! Replace must run **after** our UI hides and the source app is frontmost again.
//! The delayed restore compares `NSPasteboard.changeCount` against the value
//! recorded right after our own write and leaves the pasteboard alone if the
//! user copied something else in between.

#![allow(deprecated)]

use anyhow::{anyhow, bail, Context, Result};
use arboard::Clipboard;
use async_trait::async_trait;
use cocoa::foundation::{NSPoint, NSRect};
use core_foundation::base::{CFRange, TCFType};
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

fn read_ax_selected_text(element: &AXUIElement) -> Result<String> {
    let text: CFString = element
        .attribute(&attr_typed::<CFString>("AXSelectedText"))
        .map_err(|e| anyhow!("AXSelectedText: {e}"))?;
    Ok(text.to_string())
}

fn set_ax_selected_text(element: &AXUIElement, text: &str) -> Result<()> {
    element
        .set_attribute(
            &attr_typed::<CFString>("AXSelectedText"),
            CFString::new(text),
        )
        .map_err(|e| anyhow!("set AXSelectedText: {e}"))
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

fn set_ax_selected_range(element: &AXUIElement, location: i64, length: i64) -> Result<()> {
    let mut range = CFRange {
        location: location as isize,
        length: length as isize,
    };
    unsafe {
        let ax_ref = accessibility_sys::AXValueCreate(
            accessibility_sys::kAXValueTypeCFRange,
            &mut range as *mut _ as *const _,
        );
        if ax_ref.is_null() {
            bail!("AXValueCreate CFRange failed");
        }
        let cf = core_foundation::base::CFType::wrap_under_create_rule(ax_ref as _);
        element
            .set_attribute(
                &attr_typed::<core_foundation::base::CFType>("AXSelectedTextRange"),
                cf,
            )
            .map_err(|e| anyhow!("set AXSelectedTextRange: {e}"))
    }
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

/// Collapse the selection to its end the way a user would: a plain → press.
fn collapse_selection_right() -> Result<()> {
    let flags = CGEventFlags::empty();
    post_key(KeyCode::RIGHT_ARROW, flags, true)?;
    thread::sleep(Duration::from_millis(20));
    post_key(KeyCode::RIGHT_ARROW, flags, false)?;
    thread::sleep(Duration::from_millis(60));
    Ok(())
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

    /// Current `NSPasteboard.changeCount`; bumps on every write by any process.
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
    pub(super) fn restore_pasteboard(snap: &PasteboardSnapshot) -> Result<()> {
        let _guard = lock();
        autoreleasepool(|| {
            // SAFETY: every object we create is either autoreleased or released right
            // after the pasteboard/array retains it; NSData copies our bytes on creation.
            unsafe {
                let pb = general_pasteboard()?;
                let _: i64 = msg_send![pb, clearContents];
                if snap.items.is_empty() {
                    return Ok(());
                }
                let array: id =
                    msg_send![class!(NSMutableArray), arrayWithCapacity: snap.items.len()];
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
        })
    }
}

use pasteboard::{pasteboard_change_count, restore_pasteboard, snapshot_pasteboard};

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

fn clip_set(clip: &Mutex<Clipboard>, text: &str) -> Result<()> {
    let _pb = pasteboard::lock();
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
        let snap = snapshot_pasteboard().context("snapshot pasteboard before ⌘C")?;
        clipboard_copy()?;
        thread::sleep(Duration::from_millis(80));
        let copied = clip_get(&self.clipboard)?;
        // Put back everything that was there (all items and types), not just the text.
        if let Err(e) = restore_pasteboard(&snap) {
            warn!("restore pasteboard after ⌘C failed ({e})");
        }
        Ok(copied.filter(|t| !t.is_empty()))
    }

    fn replace_via_clipboard_fallback(&self, text: &str) -> Result<()> {
        let snap = snapshot_pasteboard().context("snapshot pasteboard before ⌘V")?;
        clip_set(&self.clipboard, text)?;
        // changeCount right after our write; any later copy by the user bumps it.
        let ours = pasteboard_change_count();
        // Let the pasteboard settle before synthesizing ⌘V.
        thread::sleep(Duration::from_millis(80));
        clipboard_paste()?;
        // Delayed restore so the target app can consume the paste.
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            let now = pasteboard_change_count();
            if now != ours {
                debug!(
                    ours,
                    now, "pasteboard changed since our paste; leaving it alone"
                );
                return;
            }
            if let Err(e) = restore_pasteboard(&snap) {
                warn!("restore pasteboard after ⌘V failed ({e})");
            } else {
                debug!(
                    items = snap.items.len(),
                    "restored pasteboard snapshot after paste"
                );
            }
        });
        Ok(())
    }

    /// Replace selection in a previously focused app. Call after hiding our UI.
    pub fn replace_in_app(
        &self,
        pid: Option<i32>,
        text: &str,
        original: &str,
        range: Option<(i64, i64)>,
    ) -> Result<()> {
        if !accessibility_trusted() {
            bail!(
                "Accessibility permission missing. Enable Selara (or the Terminal/binary \
                 you launched) under System Settings → Privacy & Security → Accessibility."
            );
        }

        let element = match pid {
            Some(pid) => focused_element_for_pid(pid).or_else(|_| focused_element()),
            None => focused_element(),
        };

        if let Ok(el) = element {
            if let Some((loc, len)) = range {
                if let Err(e) = set_ax_selected_range(&el, loc, len) {
                    debug!("restore AXSelectedTextRange failed ({e})");
                } else {
                    thread::sleep(Duration::from_millis(40));
                }
            }

            match set_ax_selected_text(&el, text) {
                Ok(()) => {
                    // Many apps (Electron, browsers) report success but do nothing.
                    let verified = read_ax_selected_text(&el)
                        .ok()
                        .map(|t| t == text)
                        .unwrap_or(false);
                    // Also treat "original selection gone / replaced" as ok when selected text
                    // is empty after a successful set (some fields clear selection after edit).
                    let changed_away_from_original = read_ax_selected_text(&el)
                        .ok()
                        .map(|t| t != original)
                        .unwrap_or(false);
                    if verified || changed_away_from_original {
                        debug!(len = text.len(), "replaced selection via AX");
                        return Ok(());
                    }
                    debug!("AX replace reported ok but text unchanged; using paste fallback");
                }
                Err(e) => debug!("AX replace failed ({e}); using clipboard paste fallback"),
            }
        }

        self.replace_via_clipboard_fallback(text)
            .context("clipboard replace fallback")
    }
}

impl MacosSelection {
    /// Insert `text` right after the selection that was captured, leaving the
    /// selection itself untouched. Call after hiding our UI and re-activating
    /// the target app.
    ///
    /// With a captured `range` the caret is moved to the end of the selection
    /// through Accessibility and the text is written there (with the same
    /// verify-then-paste fallback as a Replace). If the range cannot be applied,
    /// or none was captured (clipboard fallback read the selection), a plain →
    /// key press collapses the selection to its end and the text is pasted.
    pub fn insert_after_selection(
        &self,
        pid: Option<i32>,
        text: &str,
        range: Option<(i64, i64)>,
    ) -> Result<()> {
        if !accessibility_trusted() {
            bail!(
                "Accessibility permission missing. Enable Selara (or the Terminal/binary \
                 you launched) under System Settings → Privacy & Security → Accessibility."
            );
        }

        if let Some((loc, len)) = range {
            let caret = loc + len;
            let element = match pid {
                Some(pid) => focused_element_for_pid(pid).or_else(|_| focused_element()),
                None => focused_element(),
            };
            match element.and_then(|el| set_ax_selected_range(&el, caret, 0)) {
                Ok(()) => {
                    thread::sleep(Duration::from_millis(40));
                    return self.replace_in_app(pid, text, "", Some((caret, 0)));
                }
                Err(e) => debug!("collapse selection via AX failed ({e}); using → + paste"),
            }
        }

        collapse_selection_right()?;
        self.replace_via_clipboard_fallback(text)
            .context("clipboard insert fallback")
    }

    /// Put `original` back where a previous Replace wrote `replacement`.
    ///
    /// `range` is the location of the original selection when it was captured
    /// (from `AXSelectedTextRange`). When it is missing (the clipboard fallback
    /// read the selection) the caret is assumed to sit right after the pasted
    /// text, which is where ⌘V leaves it; if that assumption cannot be
    /// verified the caller gets an error rather than a paste in the wrong place.
    pub fn undo_replace(
        &self,
        pid: Option<i32>,
        original: &str,
        replacement: &str,
        range: Option<(i64, i64)>,
    ) -> Result<()> {
        // AX ranges are NSRange-like: UTF-16 code units.
        let replaced_len = replacement.encode_utf16().count() as i64;
        let target = match range {
            Some((loc, _)) => (loc, replaced_len),
            None => {
                let element = match pid {
                    Some(pid) => focused_element_for_pid(pid).or_else(|_| focused_element()),
                    None => focused_element(),
                }
                .context("undo: no focused element in the target app")?;
                match read_ax_selected_range(&element) {
                    Some((loc, 0)) if loc >= replaced_len => (loc - replaced_len, replaced_len),
                    Some((loc, len)) if len == replaced_len => (loc, len),
                    other => bail!(
                        "undo: cannot locate the replaced text (selection is {other:?}); \
                         use Undo (⌘Z) in the app instead"
                    ),
                }
            }
        };
        self.replace_in_app(pid, original, replacement, Some(target))
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

        match focused_element() {
            Ok(el) => {
                let range = read_ax_selected_range(&el);
                match read_ax_selected_text(&el) {
                    Ok(text) if !text.is_empty() => {
                        debug!(len = text.len(), "read selection via AX");
                        return Ok(Some(SelectionSnapshot {
                            text,
                            app_name,
                            range,
                        }));
                    }
                    Ok(_) => debug!("AX selected text empty; trying clipboard fallback"),
                    Err(e) => debug!("AX read failed ({e}); trying clipboard fallback"),
                }
            }
            Err(e) => debug!("focused element unavailable ({e}); trying clipboard fallback"),
        }

        let text = self
            .read_via_clipboard_fallback()
            .context("clipboard selection fallback")?;
        Ok(text.map(|text| SelectionSnapshot {
            text,
            app_name,
            range: None,
        }))
    }

    async fn replace_selection(&self, text: &str) -> Result<()> {
        self.replace_in_app(None, text, "", None)
    }
}

#[cfg(test)]
mod tests {
    use super::pasteboard::{
        pasteboard_change_count, restore_pasteboard, snapshot_pasteboard, PasteboardSnapshot,
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
}
