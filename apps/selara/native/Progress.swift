// Native command progress. Called only from the Rust service's AppKit thread.
// Built into the service alongside the unchanged ThinkingOrbsKit sources.
import AppKit
import ApplicationServices
import SwiftUI

// The orb is also the cancel target. Keep it available to accessibility while
// revealing its visual affordance only when the pointer enters the target.
private final class OrbCancelButton: NSButton {
    var onHover: ((Bool) -> Void)?
    var hovered = false {
        didSet {
            image = hovered ? NSImage(systemSymbolName: "xmark", accessibilityDescription: nil) : nil
            onHover?(hovered)
        }
    }
    private var tracking: NSTrackingArea?
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds, options: [.activeAlways, .inVisibleRect, .mouseEnteredAndExited], owner: self)
        addTrackingArea(area)
        tracking = area
    }
    override func mouseEntered(with event: NSEvent) { hovered = true }
    override func mouseExited(with event: NSEvent) { hovered = false }
}

private final class OrbPlayback: ObservableObject {
    @Published var paused = true
}

@available(macOS 12.0, *)
private struct WorkingOrb: View {
    @ObservedObject var playback: OrbPlayback
    var body: some View {
        ThinkingOrb(state: .working, size: .px64, theme: .light, paused: playback.paused, displaySize: 40)
            .shadow(color: .white.opacity(0.9), radius: 0.7)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}

private final class PassivePanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

// All placement coordinates are AppKit screen points (bottom-left origin).
// Prefer the editor's gutter to avoid both text and selection toolbars. Some
// browsers expose editor bounds but no usable text-range bounds.
func progressOrigin(anchor: NSRect, visible: NSRect, editor: NSRect? = nil) -> NSPoint {
    let side: CGFloat = 48, gap: CGFloat = 12
    func clamp(_ point: NSPoint) -> NSPoint {
        NSPoint(x: max(visible.minX, min(point.x, visible.maxX - side)),
                y: max(visible.minY, min(point.y, visible.maxY - side)))
    }
    let lineY = anchor.height > 0 ? anchor.maxY - min(anchor.height, side) / 2 : anchor.midY
    if let editor, editor.intersects(visible) {
        let y = max(editor.minY, min(lineY, editor.maxY)) - side / 2
        if editor.minX - gap - side >= visible.minX {
            return clamp(NSPoint(x: editor.minX - gap - side, y: y))
        }
        if editor.maxX + gap + side <= visible.maxX {
            return clamp(NSPoint(x: editor.maxX + gap, y: y))
        }
    }
    // Without an editor gutter, sit diagonally away from the saved location.
    // A wider vertical gap avoids the toolbar many editors show below selection.
    let x = anchor.minX - gap - side >= visible.minX ? anchor.minX - gap - side : anchor.maxX + gap
    let y = anchor.maxY + 24 + side <= visible.maxY ? anchor.maxY + 24 : anchor.minY - 64 - side
    return clamp(NSPoint(x: x, y: y))
}

private final class ProgressController: NSObject {
    private let panel: PassivePanel
    private let playback = OrbPlayback()
    private let indicator: NSView
    private let fallbackSpinner: NSProgressIndicator?
    private let done = NSImageView()
    private let cancelButton = OrbCancelButton()
    private var generation: UInt64 = 0
    private var running = false
    private var initiationAnchor: NSRect?
    private var initiationEditor: NSRect?
    var cancelled = false
    private var reduceMotion: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

    override init() {
        precondition(Thread.isMainThread)
        panel = PassivePanel(contentRect: NSRect(x: 0, y: 0, width: 48, height: 48),
                             styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        if #available(macOS 12.0, *) {
            indicator = NSHostingView(rootView: WorkingOrb(playback: playback))
            fallbackSpinner = nil
        } else {
            // Keep the application's existing macOS 11 minimum supported.
            let spinner = NSProgressIndicator()
            spinner.style = .spinning
            spinner.controlSize = .small
            spinner.isIndeterminate = true
            fallbackSpinner = spinner
            indicator = spinner
        }
        super.init()
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.isReleasedWhenClosed = false
        panel.hidesOnDeactivate = false
        panel.level = .floating
        panel.collectionBehavior = [.fullScreenAuxiliary, .moveToActiveSpace]
        panel.animationBehavior = .none
        panel.acceptsMouseMovedEvents = true
        panel.title = "Selara"
        let content = NSView(frame: NSRect(x: 0, y: 0, width: 48, height: 48))
        content.wantsLayer = true
        content.layer?.backgroundColor = NSColor.clear.cgColor
        panel.contentView = content
        indicator.frame = NSRect(x: 4, y: 4, width: 40, height: 40)
        content.addSubview(indicator)
        done.frame = NSRect(x: 14, y: 14, width: 20, height: 20)
        done.image = NSImage(systemSymbolName: "checkmark", accessibilityDescription: "Updated")
        done.contentTintColor = .systemGreen
        done.isHidden = true
        content.addSubview(done)
        cancelButton.frame = content.bounds
        cancelButton.title = ""
        cancelButton.imagePosition = .imageOnly
        cancelButton.imageScaling = .scaleNone
        cancelButton.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 16, weight: .medium)
        cancelButton.isBordered = false
        cancelButton.contentTintColor = .systemGray
        cancelButton.toolTip = "Cancel command (Esc)"
        cancelButton.setAccessibilityLabel("Cancel command")
        cancelButton.target = self
        cancelButton.action = #selector(cancelRun)
        cancelButton.onHover = { [weak self] hovered in
            self?.indicator.alphaValue = hovered ? 0 : 1
        }
        content.addSubview(cancelButton)
    }

    func show(_ command: String) {
        hide()
        cancelled = false
        running = true
        cancelButton.setAccessibilityLabel("\(command) running. Cancel command")
        cancelButton.hovered = false
        done.isHidden = true
        indicator.isHidden = false
        cancelButton.isHidden = false
        position()
        let token = generation
        // Fast commands finish without showing a transient flash.
        later(0.18, token) { controller in
            controller.playback.paused = controller.reduceMotion
            if !controller.reduceMotion { controller.fallbackSpinner?.startAnimation(nil) }
            controller.panel.alphaValue = controller.reduceMotion ? 1 : 0
            controller.panel.orderFrontRegardless()
            controller.fade(to: 1, duration: 0.18, token: token)
        }
    }

    func succeed() {
        guard running, panel.isVisible else { hide(); return }
        generation &+= 1
        running = false
        let token = generation
        playback.paused = true
        fallbackSpinner?.stopAnimation(nil)
        indicator.isHidden = true
        done.isHidden = false
        cancelButton.isHidden = true
        panel.alphaValue = 1
        later(0.65, token) { controller in
            controller.fade(to: 0, duration: 0.2, token: token) { $0.panel.orderOut(nil) }
        }
    }

    func hide() {
        generation &+= 1
        running = false
        cancelButton.hovered = false
        playback.paused = true
        fallbackSpinner?.stopAnimation(nil)
        panel.orderOut(nil)
        panel.alphaValue = 1
    }

    @objc private func cancelRun() {
        guard running else { return }
        hide()
        cancelled = true
        // Rust polls at 50ms while working and 16ms while restoring focus.
    }

    private func later(_ delay: TimeInterval, _ token: UInt64,
                       _ action: @escaping (ProgressController) -> Void) {
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let self, self.generation == token else { return }
            action(self)
        }
    }

    // Each alpha update checks the run token. An old fade cannot dim or hide
    // a newer command's panel; queued closures never retain the controller.
    private func fade(to target: CGFloat, duration: TimeInterval, token: UInt64,
                      completion: @escaping (ProgressController) -> Void = { _ in }) {
        if reduceMotion {
            panel.alphaValue = target
            completion(self)
            return
        }
        let start = ProcessInfo.processInfo.systemUptime
        fadeStep(from: panel.alphaValue, to: target, start: start, duration: duration,
                 token: token, completion: completion)
    }

    private func fadeStep(from: CGFloat, to: CGFloat, start: TimeInterval,
                          duration: TimeInterval, token: UInt64,
                          completion: @escaping (ProgressController) -> Void) {
        guard generation == token else { return }
        let fraction = min(1, (ProcessInfo.processInfo.systemUptime - start) / duration)
        let eased = fraction * fraction * (3 - 2 * fraction)
        panel.alphaValue = from + (to - from) * CGFloat(eased)
        if fraction >= 1 { completion(self); return }
        later(1.0 / 60.0, token) { controller in
            controller.fadeStep(from: from, to: to, start: start, duration: duration,
                                token: token, completion: completion)
        }
    }

    // Capture before the instruction/confirmation UI can move focus or the
    // pointer. This is decorative geometry only; Rust still validates selection.
    func captureAnchor(sourcePID: Int32) {
        let pointer = NSEvent.mouseLocation
        initiationAnchor = NSRect(origin: pointer, size: .zero)
        initiationEditor = nil
        guard sourcePID > 0,
              NSWorkspace.shared.frontmostApplication?.processIdentifier == sourcePID else { return }
        let app = AXUIElementCreateApplication(sourcePID)
        AXUIElementSetMessagingTimeout(app, 0.06)
        var focused: CFTypeRef?
        guard AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute as CFString, &focused) == .success,
              let focused, CFGetTypeID(focused) == AXUIElementGetTypeID() else { return }
        let element = unsafeBitCast(focused, to: AXUIElement.self)
        AXUIElementSetMessagingTimeout(element, 0.06)
        var position: CFTypeRef?
        var size: CFTypeRef?
        var origin = CGPoint.zero
        var dimensions = CGSize.zero
        if AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &position) == .success,
           AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &size) == .success,
           let position, let size,
           CFGetTypeID(position) == AXValueGetTypeID(), CFGetTypeID(size) == AXValueGetTypeID(),
           AXValueGetValue(unsafeBitCast(position, to: AXValue.self), .cgPoint, &origin),
           AXValueGetValue(unsafeBitCast(size, to: AXValue.self), .cgSize, &dimensions) {
            initiationEditor = screenRect(CGRect(origin: origin, size: dimensions))
        }
        var range: CFTypeRef?
        var selectedRange = CFRange()
        guard AXUIElementCopyAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, &range) == .success,
              let range, CFGetTypeID(range) == AXValueGetTypeID(),
              AXValueGetValue(unsafeBitCast(range, to: AXValue.self), .cfRange, &selectedRange),
              selectedRange.location >= 0, selectedRange.length >= 0 else { return }
        var bounds: CFTypeRef?
        var rect = CGRect.zero
        guard AXUIElementCopyParameterizedAttributeValue(element, kAXBoundsForRangeParameterizedAttribute as CFString, range, &bounds) == .success,
              let bounds, CFGetTypeID(bounds) == AXValueGetTypeID(),
              AXValueGetValue(unsafeBitCast(bounds, to: AXValue.self), .cgRect, &rect),
              let anchor = screenRect(rect) else { return }
        initiationAnchor = anchor
    }

    private func screenRect(_ rect: CGRect) -> NSRect? {
        guard rect.width >= 0, rect.height > 0,
              [rect.minX, rect.minY, rect.width, rect.height].allSatisfy({ $0.isFinite }),
              let primary = NSScreen.screens.first else { return nil }
        let anchor = NSRect(x: rect.minX, y: primary.frame.maxY - rect.maxY,
                            width: rect.width, height: rect.height)
        // Some apps expose off-screen document coordinates. Prefer the saved
        // pointer over a bogus or scrolled-out text location.
        guard NSScreen.screens.contains(where: { screen in
            screen.visibleFrame.intersects(anchor) || screen.visibleFrame.contains(anchor.origin)
        }) else { return nil }
        return anchor
    }

    private func position() {
        let anchor = initiationAnchor ?? NSRect(origin: NSEvent.mouseLocation, size: .zero)
        let center = NSPoint(x: anchor.midX, y: anchor.midY)
        let screens = NSScreen.screens
        let screen = screens.first(where: { $0.frame.contains(center) })
            ?? screens.max(by: { left, right in
                func area(_ screen: NSScreen) -> CGFloat {
                    let intersection = screen.visibleFrame.intersection(anchor)
                    return intersection.isEmpty ? 0 : intersection.width * intersection.height
                }
                return area(left) < area(right)
            })
            ?? NSScreen.main
        guard let screen else { return }
        panel.setFrameOrigin(progressOrigin(anchor: anchor, visible: screen.visibleFrame, editor: initiationEditor))
    }

}

// One retained handle crosses the C ABI. All access, including release, stays
// on the main thread. The Rust owner is explicitly !Send and !Sync.
private func controller(_ handle: UnsafeMutableRawPointer) -> ProgressController {
    precondition(Thread.isMainThread)
    return Unmanaged<ProgressController>.fromOpaque(handle).takeUnretainedValue()
}

@_cdecl("selara_progress_create")
public func progressCreate() -> UnsafeMutableRawPointer {
    precondition(Thread.isMainThread)
    return Unmanaged.passRetained(ProgressController()).toOpaque()
}
@_cdecl("selara_progress_destroy")
public func progressDestroy(_ handle: UnsafeMutableRawPointer) {
    controller(handle).hide()
    Unmanaged<ProgressController>.fromOpaque(handle).release()
}
@_cdecl("selara_progress_capture_anchor")
public func progressCaptureAnchor(_ handle: UnsafeMutableRawPointer, _ sourcePID: Int32) {
    controller(handle).captureAnchor(sourcePID: sourcePID)
}
@_cdecl("selara_progress_show")
public func progressShow(_ handle: UnsafeMutableRawPointer, _ title: UnsafePointer<CChar>) {
    controller(handle).show(String(cString: title))
}
@_cdecl("selara_progress_hide")
public func progressHide(_ handle: UnsafeMutableRawPointer) { controller(handle).hide() }
@_cdecl("selara_progress_succeed")
public func progressSucceed(_ handle: UnsafeMutableRawPointer) { controller(handle).succeed() }
@_cdecl("selara_progress_take_cancelled")
public func progressTakeCancelled(_ handle: UnsafeMutableRawPointer) -> Bool {
    let view = controller(handle)
    let cancelled = view.cancelled
    view.cancelled = false
    return cancelled
}
