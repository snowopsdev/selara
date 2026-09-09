//! Selection read/replace via Accessibility, with Cmd+C / Cmd+V clipboard fallback.
//!
//! Tradeoff: AX `AXSelectedText` set is preferred when the focused element supports
//! it. Many apps ignore setValue; the fallback snapshots the whole pasteboard
//! (every item and every type, not just text), pastes the result with Cmd+V,
//! then restores the snapshot after a short delay.
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

/// Did an AX insertion of `text` at `caret` actually land?
///
/// An insert cannot use the Replace heuristic ("the selection no longer holds
/// the original text"), because the text it replaces is the empty string: a
/// field that collapses its selection after an edit reports an empty selection
/// both when the write succeeded and when it silently did nothing. So look at
/// where the caret ended up instead. After a real insertion the field either
/// leaves the inserted run selected, or collapses the caret to the end of it;
/// an app that reported success and wrote nothing leaves the caret at `caret`.
///
/// `selected_range` and `caret` are AX ranges, i.e. UTF-16 code units.
fn insertion_landed(
    selected_text: Option<&str>,
    selected_range: Option<(i64, i64)>,
    text: &str,
    caret: i64,
) -> bool {
    if selected_text == Some(text) && !text.is_empty() {
        return true;
    }
    let inserted = text.encode_utf16().count() as i64;
    match selected_range {
        // Caret collapsed to the end of what we wrote.
        Some((loc, 0)) => loc == caret + inserted,
        // The inserted run is still selected.
        Some((loc, len)) => loc == caret && len == inserted,
        None => false,
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
    pub(super) fn pasteboard_text() -> Option<String> {
        let _guard = lock();
        autoreleasepool(|| {
            // SAFETY: `ty` is +1 and released here; `stringForType:` returns an
            // autoreleased NSString that `nsstring_to_string` copies out.
            unsafe {
                let pb = general_pasteboard().ok()?;
                let ty = nsstring_new(TEXT_TYPE);
                if ty.is_null() {
                    return None;
                }
                let s: id = msg_send![pb, stringForType: ty];
                let out = nsstring_to_string(s);
                let _: () = msg_send![ty, release];
                out
            }
        })
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

// There is no `clip_set`: writing the paste text goes through
// `pasteboard::write_text`, which returns the changeCount its own write
// produced. arboard cannot report that, and reading the count back afterwards
// is exactly the race the delayed restore has to be immune to.

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
        // Write and read the guard in one call: `clearContents` returns the
        // changeCount it produced, so `ours` is always Selara's own write and
        // never an intervening one from another process.
        let ours =
            pasteboard::write_text(text).context("write the paste text to the pasteboard")?;
        let written = text.to_string();
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
            // Second guard on the contents themselves, so a writer that somehow
            // leaves the count alone still cannot have its content discarded.
            if pasteboard::pasteboard_text().as_deref() != Some(written.as_str()) {
                debug!("pasteboard no longer holds our paste text; leaving it alone");
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
    /// through Accessibility and the text is written there, then verified by
    /// where the caret ended up (see `insertion_landed`) before the write is
    /// treated as done. If the range cannot be applied, or none was captured
    /// (clipboard fallback read the selection), a plain → key press collapses
    /// the selection to its end and the text is pasted.
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

        // True once AX has already put the caret at the end of the selection, so
        // the paste fallback must not press → again and skip a character.
        let mut caret_collapsed = false;

        if let Some((loc, len)) = range {
            let caret = loc + len;
            let element = match pid {
                Some(pid) => focused_element_for_pid(pid).or_else(|_| focused_element()),
                None => focused_element(),
            };
            match element {
                Ok(el) => match set_ax_selected_range(&el, caret, 0) {
                    Ok(()) => {
                        caret_collapsed = true;
                        thread::sleep(Duration::from_millis(40));
                        match set_ax_selected_text(&el, text) {
                            // Verify the insertion directly. Routing this through
                            // `replace_in_app` with an empty `original` cannot tell a
                            // real insert from a silent no-op in a field that clears
                            // its selection after an edit, and so pasted a second copy.
                            Ok(()) => {
                                let landed = insertion_landed(
                                    read_ax_selected_text(&el).ok().as_deref(),
                                    read_ax_selected_range(&el),
                                    text,
                                    caret,
                                );
                                if landed {
                                    debug!(len = text.len(), "inserted after selection via AX");
                                    return Ok(());
                                }
                                debug!("AX insert reported ok but the caret did not move; pasting");
                            }
                            Err(e) => debug!("AX insert failed ({e}); using paste fallback"),
                        }
                    }
                    Err(e) => debug!("collapse selection via AX failed ({e}); using → + paste"),
                },
                Err(e) => debug!("no focused element for insert ({e}); using → + paste"),
            }
        }

        if !caret_collapsed {
            collapse_selection_right()?;
        }
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
    /// Where the replaced text sits: the captured range when we have one,
    /// otherwise derived from the caret, which ⌘V leaves just after the paste.
    fn undo_target(
        range: Option<(i64, i64)>,
        selection: Option<(i64, i64)>,
        replaced_len: i64,
    ) -> Result<(i64, i64)> {
        if let Some((loc, _)) = range {
            return Ok((loc, replaced_len));
        }
        match selection {
            Some((loc, 0)) if loc >= replaced_len => Ok((loc - replaced_len, replaced_len)),
            Some((loc, len)) if len == replaced_len => Ok((loc, len)),
            other => bail!(
                "undo: cannot locate the replaced text (selection is {other:?}); \
                 use Undo (⌘Z) in the app instead"
            ),
        }
    }

    /// Put `original` back where a previous Replace wrote `replacement`.
    ///
    /// Undo only ever overwrites text it can prove is still its own: it acts on
    /// the process that received the replacement (never on whatever happens to
    /// be focused now), and it re-reads the target range and refuses unless the
    /// contents still equal `replacement`. There is deliberately no clipboard
    /// paste fallback here: a paste cannot be verified, and guessing would mean
    /// overwriting text the user wrote after the replacement.
    pub fn undo_replace(
        &self,
        pid: Option<i32>,
        original: &str,
        replacement: &str,
        range: Option<(i64, i64)>,
    ) -> Result<()> {
        if !accessibility_trusted() {
            bail!(
                "Accessibility permission missing. Enable Selara (or the Terminal/binary \
                 you launched) under System Settings → Privacy & Security → Accessibility."
            );
        }
        // Strict: if the app that received the replacement is gone, undoing
        // into the current frontmost app would corrupt an unrelated document.
        let element = match pid {
            Some(pid) => focused_element_for_pid(pid)
                .context("undo: the app that received the replacement is no longer available")?,
            None => focused_element().context("undo: no focused element")?,
        };

        // AX ranges are NSRange-like: UTF-16 code units.
        let replaced_len = replacement.encode_utf16().count() as i64;
        let (loc, len) = Self::undo_target(range, read_ax_selected_range(&element), replaced_len)?;
        set_ax_selected_range(&element, loc, len)
            .context("undo: cannot select the replaced text")?;
        thread::sleep(Duration::from_millis(40));

        let current = read_ax_selected_text(&element)
            .context("undo: cannot read the text to restore over")?;
        if current != replacement {
            bail!(
                "undo: the text changed since the replacement, so Selara will not \
                 overwrite it; use Undo (⌘Z) in the app instead"
            );
        }

        set_ax_selected_text(&element, original)
            .context("undo: could not write the original text back")?;
        // Some apps report success without changing anything; an emptied
        // selection is a normal post-edit state and counts as applied.
        let after = read_ax_selected_text(&element).unwrap_or_default();
        if !after.is_empty() && after != original {
            bail!("undo: the app did not accept the restored text; use Undo (⌘Z) instead");
        }
        debug!(len = original.len(), "restored original via AX undo");
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
    use super::MacosSelection;

    #[test]
    fn undo_target_prefers_the_captured_range() {
        // The captured location wins; the length always comes from what was written.
        let t = MacosSelection::undo_target(Some((10, 3)), Some((99, 99)), 7).unwrap();
        assert_eq!(t, (10, 7));
    }

    #[test]
    fn undo_target_derives_from_a_caret_after_the_paste() {
        // ⌘V leaves the caret just past the pasted text.
        let t = MacosSelection::undo_target(None, Some((20, 0)), 5).unwrap();
        assert_eq!(t, (15, 5));
    }

    #[test]
    fn undo_target_accepts_a_selection_of_the_written_length() {
        let t = MacosSelection::undo_target(None, Some((4, 6)), 6).unwrap();
        assert_eq!(t, (4, 6));
    }

    #[test]
    fn undo_target_refuses_when_the_caret_moved() {
        // Caret before the paste could even fit, a wrong-length selection, or
        // no selection at all: all unverifiable, so refuse rather than guess.
        assert!(MacosSelection::undo_target(None, Some((2, 0)), 5).is_err());
        assert!(MacosSelection::undo_target(None, Some((4, 3)), 6).is_err());
        assert!(MacosSelection::undo_target(None, None, 5).is_err());
    }

    use super::insertion_landed;
    use super::pasteboard::{
        pasteboard_change_count, pasteboard_text, restore_pasteboard, snapshot_pasteboard,
        write_text, PasteboardSnapshot,
    };
    use arboard::Clipboard;
    use std::sync::{Mutex, MutexGuard};

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

    /// The regression this guards: a field that clears its selection after an
    /// AX edit reports an empty selection, which the Replace heuristic reads as
    /// "nothing happened" and follows with a ⌘V, inserting the text twice.
    #[test]
    fn insertion_is_recognised_when_the_field_clears_its_selection() {
        assert!(insertion_landed(Some(""), Some((17, 0)), "added", 12));
    }

    #[test]
    fn insertion_is_recognised_when_the_field_keeps_it_selected() {
        assert!(insertion_landed(Some("added"), Some((12, 5)), "added", 12));
    }

    #[test]
    fn a_silent_no_op_is_not_mistaken_for_an_insertion() {
        // App reported success, wrote nothing: caret still sits at the collapse
        // point and the selection is empty. Must fall through to the paste.
        assert!(!insertion_landed(Some(""), Some((12, 0)), "added", 12));
        assert!(!insertion_landed(None, Some((12, 0)), "added", 12));
        assert!(!insertion_landed(None, None, "added", 12));
    }

    #[test]
    fn insertion_length_is_measured_in_utf16_code_units() {
        // "🙂" is one char but two UTF-16 code units, which is what AX ranges use.
        assert!(insertion_landed(Some(""), Some((14, 0)), "🙂", 12));
        assert!(!insertion_landed(Some(""), Some((13, 0)), "🙂", 12));
    }
}
