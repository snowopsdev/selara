//! A progress panel that never becomes key or activates Selara.
//!
//! Winit's macOS `set_visible(true)` calls `makeKeyAndOrderFront`, even for
//! an initially inactive window. Keep progress separate from the egui dialogs.

#![allow(deprecated)] // Match the existing macOS backend until its objc2 migration.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use cocoa::base::{id, nil};
use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

pub struct ProgressPanel {
    panel: id,
    label: id,
    target: id,
    cancelled: Arc<AtomicBool>,
}

extern "C" fn cancel(target: &Object, _: Sel, _: id) {
    // SAFETY: the ivar points to this panel's retained Arc until its target is
    // released on the same main thread in Drop.
    unsafe {
        let flag = (*target.get_ivar::<*const std::ffi::c_void>("cancelFlag")).cast::<AtomicBool>();
        if let Some(flag) = flag.as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

fn target_class() -> &'static Class {
    static CLASS: OnceLock<&'static Class> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut class = ClassDecl::new("SelaraProgressCancelTarget", class!(NSObject))
            .expect("unique progress target class");
        class.add_ivar::<*const std::ffi::c_void>("cancelFlag");
        // SAFETY: selector and callback have matching Objective-C signatures.
        unsafe {
            class.add_method(sel!(cancel:), cancel as extern "C" fn(&Object, Sel, id));
        }
        class.register()
    })
}

impl ProgressPanel {
    /// Must be created, used, and dropped on the app's main thread.
    pub fn new() -> Self {
        assert!(unsafe {
            let main: bool = msg_send![class!(NSThread), isMainThread];
            main
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        // SAFETY: all Cocoa objects are created and retained on the main thread.
        // The panel retains its content controls; we retain panel and target.
        unsafe {
            let panel: id = msg_send![class!(NSPanel), alloc];
            let panel: id = msg_send![panel,
                initWithContentRect: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(340.0, 110.0))
                styleMask: (1u64 | (1u64 << 7))
                backing: 2u64 defer: false]; // titled, nonactivating; buffered
            let _: () = msg_send![panel, setReleasedWhenClosed: false];
            let _: () = msg_send![panel, setFloatingPanel: true];
            let _: () = msg_send![panel, setHidesOnDeactivate: false];
            let _: () = msg_send![panel, setBecomesKeyOnlyIfNeeded: true];
            let title = NSString::alloc(nil).init_str("Selara");
            let _: () = msg_send![panel, setTitle: title];
            let _: () = msg_send![title, release];
            let content: id = msg_send![panel, contentView];
            let label: id = msg_send![class!(NSTextField), alloc];
            let label: id = msg_send![label, initWithFrame: NSRect::new(NSPoint::new(16.0, 52.0), NSSize::new(308.0, 44.0))];
            let _: () = msg_send![label, setEditable: false];
            let _: () = msg_send![label, setSelectable: false];
            let _: () = msg_send![label, setBezeled: false];
            let _: () = msg_send![label, setDrawsBackground: false];
            let _: () = msg_send![content, addSubview: label];
            let _: () = msg_send![label, release];
            let target: id = msg_send![target_class(), new];
            (*target).set_ivar(
                "cancelFlag",
                Arc::as_ptr(&cancelled).cast::<std::ffi::c_void>(),
            );
            let button: id = msg_send![class!(NSButton), alloc];
            let button: id = msg_send![button, initWithFrame: NSRect::new(NSPoint::new(210.0, 12.0), NSSize::new(114.0, 32.0))];
            let text = NSString::alloc(nil).init_str("Cancel (Esc)");
            let _: () = msg_send![button, setTitle: text];
            let _: () = msg_send![text, release];
            let _: () = msg_send![button, setBezelStyle: 1u64];
            let _: () = msg_send![button, setTarget: target];
            let _: () = msg_send![button, setAction: sel!(cancel:)];
            let _: () = msg_send![content, addSubview: button];
            let _: () = msg_send![button, release];
            Self {
                panel,
                label,
                target,
                cancelled,
            }
        }
    }

    pub fn show(&self, title: &str) {
        self.cancelled.store(false, Ordering::SeqCst);
        self.update(title, 0);
        unsafe {
            // SAFETY: main-thread panel access; AppKit coordinates are bottom-up.
            let point: NSPoint = msg_send![class!(NSEvent), mouseLocation];
            let screens: id = msg_send![class!(NSScreen), screens];
            let count: usize = msg_send![screens, count];
            for index in 0..count {
                let screen: id = msg_send![screens, objectAtIndex: index];
                let frame: NSRect = msg_send![screen, frame];
                if point.x >= frame.origin.x
                    && point.x < frame.origin.x + frame.size.width
                    && point.y >= frame.origin.y
                    && point.y < frame.origin.y + frame.size.height
                {
                    let visible: NSRect = msg_send![screen, visibleFrame];
                    let x = (point.x + 12.0).clamp(
                        visible.origin.x,
                        (visible.origin.x + visible.size.width - 340.0).max(visible.origin.x),
                    );
                    let top = visible.origin.y + visible.size.height;
                    let y = (point.y - 12.0).clamp((visible.origin.y + 140.0).min(top), top);
                    let _: () = msg_send![self.panel, setFrameTopLeftPoint: NSPoint::new(x, y)];
                    break;
                }
            }
            let _: () = msg_send![self.panel, orderFrontRegardless];
        }
    }

    pub fn update(&self, title: &str, chars: usize) {
        unsafe {
            let value =
                NSString::alloc(nil).init_str(&format!("{title}\n{chars} characters received…"));
            let _: () = msg_send![self.label, setStringValue: value];
            let _: () = msg_send![value, release];
        }
    }
    pub fn hide(&self) {
        unsafe {
            let _: () = msg_send![self.panel, orderOut: nil];
        }
    }
    pub fn take_cancelled(&self) -> bool {
        self.cancelled.swap(false, Ordering::SeqCst)
    }
}
impl Drop for ProgressPanel {
    fn drop(&mut self) {
        self.hide();
        unsafe {
            let _: () = msg_send![self.panel, release];
            let _: () = msg_send![self.target, release];
        }
    }
}
