// Native command progress. Called only from the Rust service's AppKit thread.
// Built into the service alongside the unchanged ThinkingOrbsKit sources.
// Every run gets the Ink Sweep over the selected lines (OverlaySweep.swift)
// when Accessibility reports plausible line bounds, and today's orb when it
// does not. Review commands get the Ghost Diff card (OverlayReview.swift).
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

final class ProgressController: NSObject {
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
    private(set) var capture: SelectionCapture?
    let sweep = InkSweep()
    let card = ReviewCard()
    enum Presentation: Equatable { case none, orb, sweep, review }
    private(set) var presentation = Presentation.none
    var cancelled = false
    var reviewAction = ReviewActionCode.none
    private var reduceMotion: Bool { Motion.reduceMotion }
    /// Forces light/dark in the preview harness; production follows the system.
    var appearanceOverride: NSAppearance? {
        didSet {
            sweep.appearance = appearanceOverride
            card.appearance = appearanceOverride
            panel.appearance = appearanceOverride
        }
    }

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
        sweep.onCancel = { [weak self] in self?.cancelRun() }
        card.onAction = { [weak self] code in self?.reviewAction = code }
    }

    /// Bounds of the replaced text after a verified paste. The preview
    /// substitutes its own text view; production asks Accessibility.
    var replacedLinesProvider: (SelectionCapture, Int, Int) -> [NSRect]? = { capture, location, length in
        SelectionGeometry.replacedLines(pid: capture.pid, location: location, length: length,
                                        element: capture.element)
    }

    /// Lines for the Ink Sweep, or nil when the orb fallback applies.
    var sweepLines: [NSRect]? { capture?.geometry.lines }

    func show(_ command: String) {
        hide()
        cancelled = false
        running = true
        let token = generation
        if let lines = sweepLines {
            presentation = .sweep
            // Fast commands finish without showing a transient flash.
            later(Motion.fastCommandDelay, token) { controller in
                controller.sweep.showWorking(lines: lines, command: command)
            }
            return
        }
        presentation = .orb
        cancelButton.setAccessibilityLabel("\(command) running. Cancel command")
        cancelButton.hovered = false
        done.isHidden = true
        indicator.isHidden = false
        cancelButton.isHidden = false
        position()
        // Fast commands finish without showing a transient flash.
        later(Motion.fastCommandDelay, token) { controller in
            controller.playback.paused = controller.reduceMotion
            if !controller.reduceMotion { controller.fallbackSpinner?.startAnimation(nil) }
            controller.panel.alphaValue = controller.reduceMotion ? 1 : 0
            controller.panel.orderFrontRegardless()
            controller.fade(to: 1, duration: 0.18, token: token)
        }
    }

    /// Ghost Diff skeleton for a review command. A visible card (another take)
    /// switches back to its skeleton immediately.
    func showReview(_ command: String, model: String) {
        if presentation == .review, card.isVisible {
            cancelled = false
            running = true
            card.showSkeleton(title: command, model: model, anchor: reviewAnchor, pointX: reviewPointX, delay: 0)
            return
        }
        hide()
        cancelled = false
        running = true
        presentation = .review
        card.showSkeleton(title: command, model: model, anchor: reviewAnchor, pointX: reviewPointX,
                          delay: Motion.fastCommandDelay)
    }

    private var reviewAnchor: NSRect {
        sweepLines.flatMap(SelectionGeometry.union) ?? initiationAnchor
            ?? NSRect(origin: NSEvent.mouseLocation, size: .zero)
    }

    private var reviewPointX: CGFloat {
        if let first = sweepLines?.first { return first.minX + min(30, first.width / 2) }
        return reviewAnchor.minX + min(30, reviewAnchor.width / 2)
    }

    /// Render the diff; the card becomes key so ↩ ⇥ ⌘C Esc reach it.
    func reviewResult(_ json: String) -> Bool {
        guard presentation == .review, let payload = DiffPayload.decode(json) else { return false }
        running = false
        return card.showDiff(payload)
    }

    /// Close the card without replacing. After ⌘C a “Copied” receipt stays.
    func closeReview(copied: Bool) {
        let lines = sweepLines ?? [reviewAnchor]
        hide(animatedCard: true)
        if copied { sweep.showReceipt(.copied, lines: lines) }
    }

    /// Verified replacement. `location`/`length` are the replaced text's
    /// Accessibility range (UTF-16), or negative when unknown.
    func succeed(location: Int = -1, length: Int = -1) {
        switch presentation {
        case .sweep, .review:
            let wasReview = presentation == .review
            // A fast command never showed its sweep; keep it flash-free.
            guard wasReview || sweep.isShowingWork else { hide(); return }
            let original = sweepLines.flatMap(SelectionGeometry.union).map { [$0] }
            let replaced = capture.flatMap { replacedLinesProvider($0, location, length) }
            hide(animatedCard: true)
            if let lines = replaced ?? original {
                sweep.succeed(lines: lines)
            } else if wasReview {
                flashCheck()
            }
        case .orb, .none:
            guard running, panel.isVisible else { hide(); return }
            showCheck()
        }
    }

    private func showCheck() {
        generation &+= 1
        running = false
        presentation = .none
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

    /// Success acknowledgement when the review card had no selection bounds.
    private func flashCheck() {
        position()
        panel.orderFrontRegardless()
        showCheck()
    }

    func hide() { hide(animatedCard: false) }

    private func hide(animatedCard: Bool) {
        generation &+= 1
        running = false
        presentation = .none
        cancelButton.hovered = false
        playback.paused = true
        fallbackSpinner?.stopAnimation(nil)
        panel.orderOut(nil)
        panel.alphaValue = 1
        sweep.hide()
        if card.state != .hidden || card.isVisible { card.close(animated: animatedCard) }
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
        capture = nil
        guard sourcePID > 0,
              NSWorkspace.shared.frontmostApplication?.processIdentifier == sourcePID,
              let element = SelectionGeometry.focusedElement(pid: sourcePID) else { return }
        if let frame = SelectionGeometry.frame(of: element) {
            initiationEditor = screenRect(frame)
        }
        let capture = SelectionGeometry.capture(pid: sourcePID, element: element)
        self.capture = capture
        if let lines = capture.geometry.lines, let union = SelectionGeometry.union(lines) {
            initiationAnchor = union
            return
        }
        guard let range = capture.range,
              let rect = SelectionGeometry.bounds(of: element, range: range),
              let anchor = screenRect(rect) else { return }
        initiationAnchor = anchor
    }

    /// Preview and tests: supply geometry as if Accessibility had reported it.
    func injectCapture(lines: [NSRect], element: NSRect?, pid: pid_t = 0) {
        let geometry = SelectionGeometry.decide(lines: lines, lineCount: lines.count, element: element,
                                                screens: SelectionGeometry.screenFrames, isWebArea: false)
        capture = SelectionCapture(pid: pid, range: nil, element: element, isWebArea: false, geometry: geometry)
        initiationAnchor = SelectionGeometry.union(lines) ?? initiationAnchor
        initiationEditor = element
    }

    private func screenRect(_ rect: CGRect) -> NSRect? {
        guard rect.width >= 0, rect.height > 0,
              [rect.minX, rect.minY, rect.width, rect.height].allSatisfy({ $0.isFinite }),
              !NSScreen.screens.isEmpty else { return nil }
        let anchor = SelectionGeometry.appKitRect(rect, primaryMaxY: SelectionGeometry.primaryMaxY)
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
func progressController(_ handle: UnsafeMutableRawPointer) -> ProgressController {
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
    progressController(handle).hide()
    Unmanaged<ProgressController>.fromOpaque(handle).release()
}
@_cdecl("selara_progress_capture_anchor")
public func progressCaptureAnchor(_ handle: UnsafeMutableRawPointer, _ sourcePID: Int32) {
    progressController(handle).captureAnchor(sourcePID: sourcePID)
}
@_cdecl("selara_progress_show")
public func progressShow(_ handle: UnsafeMutableRawPointer, _ title: UnsafePointer<CChar>) {
    progressController(handle).show(String(cString: title))
}
@_cdecl("selara_progress_show_review")
public func progressShowReview(_ handle: UnsafeMutableRawPointer, _ title: UnsafePointer<CChar>,
                               _ model: UnsafePointer<CChar>) {
    progressController(handle).showReview(String(cString: title), model: String(cString: model))
}
@_cdecl("selara_progress_review_result")
public func progressReviewResult(_ handle: UnsafeMutableRawPointer, _ json: UnsafePointer<CChar>) -> Bool {
    progressController(handle).reviewResult(String(cString: json))
}
@_cdecl("selara_progress_close_review")
public func progressCloseReview(_ handle: UnsafeMutableRawPointer, _ copied: Bool) {
    progressController(handle).closeReview(copied: copied)
}
@_cdecl("selara_progress_take_review_action")
public func progressTakeReviewAction(_ handle: UnsafeMutableRawPointer) -> Int32 {
    let view = progressController(handle)
    let action = view.reviewAction
    view.reviewAction = .none
    return action.rawValue
}
@_cdecl("selara_progress_hide")
public func progressHide(_ handle: UnsafeMutableRawPointer) { progressController(handle).hide() }
@_cdecl("selara_progress_succeed")
public func progressSucceed(_ handle: UnsafeMutableRawPointer) { progressController(handle).succeed() }
@_cdecl("selara_progress_succeed_range")
public func progressSucceedRange(_ handle: UnsafeMutableRawPointer, _ location: Int64, _ length: Int64) {
    progressController(handle).succeed(location: Int(clamping: location), length: Int(clamping: length))
}
/// Whether Selara is the active app. A key non-activating panel (the review
/// card) can leave Selara active without changing NSWorkspace's frontmost
/// app, and keystrokes, including a posted ⌘V, then still reach Selara.
@_cdecl("selara_progress_app_is_active")
public func progressAppIsActive() -> Bool {
    precondition(Thread.isMainThread)
    return NSApplication.shared.isActive
}
/// Give activation back to the app that had it before Selara (the source).
/// No-op when Selara is not active, so it never moves focus on its own.
@_cdecl("selara_progress_app_deactivate")
public func progressAppDeactivate() {
    precondition(Thread.isMainThread)
    if NSApplication.shared.isActive { NSApplication.shared.deactivate() }
}
@_cdecl("selara_progress_take_cancelled")
public func progressTakeCancelled(_ handle: UnsafeMutableRawPointer) -> Bool {
    let view = progressController(handle)
    let cancelled = view.cancelled
    view.cancelled = false
    return cancelled
}
