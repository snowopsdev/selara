// Native geometry and lifecycle regressions: compile with Progress.swift and the orb sources.
import AppKit

/// Enforces a test condition even when Swift assertions are disabled under `-O`.
func expect(_ condition: @autoclosure () -> Bool, _ message: String = "", line: UInt = #line) {
    guard condition() else {
        fputs("FAIL at line \(line): \(message)\n", stderr)
        exit(1)
    }
}

/// Checks the shipped working-orb engine against thinking-orbs v0.3.1 golden samples.
///
/// The samples cover the `working64` preset at revision
/// bd204b73c9b6660fad7210b1ad48d9dc2adbb89d, including its clock multiplier.
/// They use the upstream Swift port's 1e-4 tolerance for platform trig differences.
func checkWorkingOrbGeometry() {
    let preset = resolvePreset(.working, .px64)
    let samples: [(Double, [(Int, [Double])])] = [
        (0.6, [
            (0, [30.85074971902107, 30.691517973153513, -24.109923868135848, 0.35618673453649413, 0.72, 0.20038979159114176]),
            (11, [32.54824772734648, 43.05608812199291, -21.92097341042544, 0.508906418494157, 0.2881905818429647, 1.0]),
            (249, [25.967298195033628, 10.274430608010768, -0.52241000699489, 0.784192429387529, 0.192547929984002, 1.0]),
            (515, [33.14925028097893, 33.30848202684649, 24.10992386813585, 0.35618673453649413, 0.72, 0.4996102084088583]),
        ]),
        (1.7, [
            (1, [30.58939199395569, 32.17651539010626, -24.130900474714085, 0.4754636452459916, 0.2998096085729914, 1.0]),
            (274, [24.08677412883976, 46.77302833569307, 1.115720964323365, 0.812557777139638, 0.182692953804467, 1.0]),
            (515, [31.555783460568627, 31.42658607208282, 24.161854202100955, 0.35618673453649413, 0.72, 0.49993245364404665]),
        ]),
    ]
    for (seconds, expectedDots) in samples {
        let frame = orbFrame(preset, size: 64, t: seconds * preset.speed)
        expect(frame.dots.count == 516, "working64 dot count at \(seconds)s")
        expect(frame.lines.isEmpty, "working64 should contain dots only")
        for (index, expected) in expectedDots {
            let dot = frame.dots[index]
            let actual = [dot.x, dot.y, dot.z, dot.r, dot.white, dot.a]
            for (field, values) in zip(actual, expected).enumerated() {
                expect(abs(values.0 - values.1) < 0.0001,
                       "working64 at \(seconds)s dot \(index) field \(field): \(values.0) != \(values.1)")
            }
        }
        for dot in frame.dots {
            expect([dot.x, dot.y, dot.z, dot.r, dot.white, dot.a].allSatisfy { $0.isFinite }, "finite working64 geometry")
            expect((0...64).contains(dot.x) && (0...64).contains(dot.y), "working64 stays inside its canvas")
            expect(dot.r >= 0.3 && (0...1).contains(dot.white) && (0...1).contains(dot.a), "working64 dot styling")
        }
        for (far, near) in zip(frame.dots, frame.dots.dropFirst()) {
            expect(far.z <= near.z, "working64 draws from far to near")
        }
    }
    print("Working orb geometry matches upstream golden samples")
}

/// Ink Sweep plausibility (orb fallback decision), coordinate conversion,
/// overlay placement, and the review payload. Pure: no windows.
func checkOverlayGeometry() {
    let screen = NSRect(x: 0, y: 0, width: 1440, height: 900)
    let element = NSRect(x: 100, y: 300, width: 600, height: 300)
    let line1 = NSRect(x: 120, y: 500, width: 520, height: 18)
    let line2 = NSRect(x: 120, y: 482, width: 300, height: 18)
    func decide(_ lines: [NSRect], count: Int? = nil, element: NSRect? = element,
                web: Bool = false) -> SweepGeometry {
        SelectionGeometry.decide(lines: lines, lineCount: count ?? lines.count, element: element,
                                 screens: [screen], isWebArea: web)
    }
    expect(decide([line2, line1]) == .lines([line1, line2]), "plausible lines are kept, top line first")
    expect(decide([line1, NSRect(x: 640, y: 500, width: 0, height: 18), line2]) == .lines([line1, line2]),
           "zero-width line-break fragments are skipped")
    expect(decide([]) == .fallback(.noBounds), "no bounds falls back to the orb")
    expect(decide([line1], web: true) == .fallback(.webArea), "web areas misreport bounds")
    expect(decide([line1], count: 13) == .fallback(.tooManyLines), "more than 12 lines falls back")
    expect(decide(Array(repeating: line1, count: 12)) == .lines(Array(repeating: line1, count: 12)), "12 lines still sweep")
    expect(decide([NSRect(x: 120, y: 500, width: 0, height: 0)]) == .fallback(.emptyRect), "zero size falls back")
    expect(decide([NSRect(x: 120, y: 500, width: 0, height: 18)]) == .fallback(.emptyRect), "only line breaks falls back")
    expect(decide([NSRect(x: 120, y: 500, width: -4, height: 18)]) == .fallback(.emptyRect), "negative size falls back")
    expect(decide([NSRect(x: CGFloat.nan, y: 500, width: 40, height: 18)]) == .fallback(.notFinite), "NaN falls back")
    expect(decide([NSRect(x: 120, y: 300, width: 500, height: 280)], element: nil) == .fallback(.tooTall),
           "a whole-element rect is not a line")
    expect(decide([NSRect(x: 50, y: 500, width: 900, height: 18)]) == .fallback(.largerThanElement),
           "bounds wider than the focused element fall back")
    expect(decide([NSRect(x: 900, y: 100, width: 100, height: 18)]) == .fallback(.largerThanElement),
           "bounds outside the focused element fall back")
    expect(decide([NSRect(x: 2000, y: 500, width: 100, height: 18)], element: nil) == .fallback(.offScreen),
           "bounds off every screen fall back")
    expect(decide([line1], element: nil) == .lines([line1]), "element frame is optional")
    expect(SelectionGeometry.appKitRect(CGRect(x: 10, y: 100, width: 50, height: 20), primaryMaxY: 900)
           == NSRect(x: 10, y: 780, width: 50, height: 20), "AX top-left converts to AppKit bottom-left")

    let visible = NSRect(x: 0, y: 0, width: 1440, height: 875)
    let chip = NSSize(width: 160, height: 21)
    expect(SelectionGeometry.chipOrigin(size: chip, lines: [line1, line2], visible: visible) == NSPoint(x: 120, y: 522),
           "chip sits 4 pt above the first line, left-aligned")
    let topLine = NSRect(x: 120, y: 860, width: 300, height: 15)
    expect(SelectionGeometry.chipOrigin(size: chip, lines: [topLine], visible: visible) == NSPoint(x: 120, y: 835),
           "chip flips below near the top of the display")
    expect(SelectionGeometry.chipOrigin(size: chip, lines: [NSRect(x: 1400, y: 500, width: 30, height: 18)], visible: visible).x
           == 1280, "chip stays on screen at the right edge")
    let hint = NSSize(width: 170, height: 21)
    expect(SelectionGeometry.hintOrigin(size: hint, lines: [line1, line2], visible: visible) == NSPoint(x: 250, y: 455),
           "receipt ends where the replaced text ends, below it")
    expect(SelectionGeometry.hintOrigin(size: hint, lines: [NSRect(x: 120, y: 10, width: 40, height: 18)], visible: visible)
           == NSPoint(x: 120, y: 34), "receipt flips above near the bottom")

    let card = NSSize(width: 470, height: 130)
    let below = SelectionGeometry.cardPlacement(size: card, anchor: line1.union(line2), pointX: 150, visible: visible)
    expect(below.below && below.origin == NSPoint(x: 104, y: 348) && below.arrowX == 46,
           "card sits below the selection with its arrow at the first line: \(below)")
    let low = NSRect(x: 600, y: 60, width: 300, height: 36)
    let above = SelectionGeometry.cardPlacement(size: card, anchor: low, pointX: 630, visible: visible)
    expect(!above.below && above.origin.y == 100, "card flips above when there is no room below: \(above)")
    let edge = SelectionGeometry.cardPlacement(size: card, anchor: NSRect(x: 1380, y: 500, width: 50, height: 18),
                                               pointX: 1400, visible: visible)
    expect(edge.origin.x == 962 && edge.arrowX == 438, "card clamps to the screen and its arrow follows: \(edge)")

    let json = #"{"segments":[{"op":"delete","text":"Hello"},{"op":"insert","text":"Hi"},{"op":"equal","text":" world"}],"delta":-3,"result":"Hi world"}"#
    let payload = DiffPayload.decode(json)
    expect(payload?.segments.count == 3 && payload?.segments[0].op == .delete && payload?.result == "Hi world",
           "review payload decodes")
    expect(payload?.deltaLabel == "−3 chars", "negative delta label")
    expect(DiffPayload(segments: [], delta: 1, result: "").deltaLabel == "+1 char", "positive singular delta label")
    expect(DiffPayload(segments: [], delta: 0, result: "").deltaLabel == "Same length", "unchanged length label")
    expect(DiffPayload.decode("{") == nil, "invalid payload is rejected")
    expect(Motion.micro == 0.12 && Motion.panel == 0.24 && Motion.spring == 0.42 && Motion.stagger == 0.038
           && Motion.hold == 1.8 && Motion.instant == 0, "motion tokens match the contract")
    print("Overlay geometry, fallback decisions, placement, and payload checks passed")
}

checkWorkingOrbGeometry()
checkOverlayGeometry()
if CommandLine.arguments.contains("--geometry-only") { exit(0) }

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
/// Selara's panels must never make this process the frontmost app (what the
/// service checks before replacing). Compare against our own pid rather than
/// the app frontmost at launch: on a shared desktop the user may switch apps
/// while the test runs. A key non-activating panel can set `NSApp.isActive`
/// without taking the menu bar or frontmost status, so that is not checked.
func notActivated() -> Bool {
    NSWorkspace.shared.frontmostApplication?.processIdentifier != ProcessInfo.processInfo.processIdentifier
}
/// Processes AppKit events for the requested test interval.
func pump(_ seconds: TimeInterval) {
    let until = Date().addingTimeInterval(seconds)
    while Date() < until {
        while let event = app.nextEvent(matching: .any, until: Date(), inMode: .default, dequeue: true) {
            app.sendEvent(event)
        }
        RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    }
}
/// Returns the visible Selara progress panel, when one exists.
func visiblePanel() -> NSWindow? { app.windows.first { $0.title == "Selara" && $0.isVisible } }
/// Shows a progress handle with the supplied command title.
func show(_ handle: UnsafeMutableRawPointer, _ title: String = "RewritePro") {
    title.withCString { progressShow(handle, $0) }
}
/// Finds the first button in a native view hierarchy.
func findButton(_ view: NSView?) -> NSButton? {
    guard let view else { return nil }
    if let button = view as? NSButton { return button }
    return view.subviews.lazy.compactMap { findButton($0) }.first
}
let display = NSRect(x: 0, y: 0, width: 1000, height: 800)
expect(progressOrigin(anchor: NSRect(x: 400, y: 300, width: 200, height: 20), visible: display, editor: NSRect(x: 100, y: 100, width: 700, height: 500)) == NSPoint(x: 40, y: 286), "stay outside the editor and its selection toolbar")
expect(progressOrigin(anchor: NSRect(x: 400, y: 300, width: 0, height: 0), visible: display, editor: NSRect(x: 100, y: 100, width: 700, height: 500)) == NSPoint(x: 40, y: 276), "use editor gutter when browser text-range bounds are missing")
expect(progressOrigin(anchor: NSRect(x: 400, y: 300, width: 200, height: 20), visible: display, editor: NSRect(x: 0, y: 100, width: 700, height: 500)) == NSPoint(x: 712, y: 286), "use right gutter when the left gutter is off screen")
expect(progressOrigin(anchor: NSRect(x: 100, y: 300, width: 200, height: 20), visible: display) == NSPoint(x: 40, y: 344), "offset away from selection when no editor bounds exist")
expect(progressOrigin(anchor: NSRect(x: 100, y: 20, width: 200, height: 20), visible: display) == NSPoint(x: 40, y: 64), "fit above near bottom")
expect(progressOrigin(anchor: NSRect(x: 970, y: 300, width: 0, height: 0), visible: display) == NSPoint(x: 910, y: 324), "keep cursor fallback inside right edge")
expect(progressOrigin(anchor: NSRect(x: 100, y: 0, width: 200, height: 780), visible: display) == NSPoint(x: 40, y: 0), "avoid covering a tall selection")
let secondDisplay = NSRect(x: -1200, y: -900, width: 1200, height: 900)
expect(progressOrigin(anchor: NSRect(x: -1170, y: -890, width: 100, height: 20), visible: secondDisplay) == NSPoint(x: -1058, y: -846), "handle negative display coordinates")
let handle = progressCreate()
progressCaptureAnchor(handle, 0)
show(handle)
progressSucceed(handle)
pump(0.4)
expect(visiblePanel() == nil, "fast commands must not flash")
show(handle)
pump(0.4)
let shown = visiblePanel()!
expect(shown.frame.size == NSSize(width: 48, height: 48))
expect(!shown.isOpaque && shown.backgroundColor == .clear && !shown.hasShadow)
expect(!shown.contentView!.subviews.contains { $0 is NSTextField }, "no labels should overlap the document")
expect(!shown.canBecomeKey && !shown.canBecomeMain)
expect(notActivated(), "progress stole focus")
let cancel = findButton(shown.contentView)!
expect(cancel.image == nil, "cancel affordance should be quiet until hovered")
let originalPointer = NSEvent.mouseLocation
let primaryTop = NSScreen.screens[0].frame.maxY
/// Posts a synthetic mouse event at an AppKit screen coordinate.
func mouse(_ type: CGEventType, _ point: NSPoint) {
    CGEvent(mouseEventSource: nil, mouseType: type,
            mouseCursorPosition: CGPoint(x: point.x, y: primaryTop - point.y),
            mouseButton: .left)!.post(tap: .cghidEventTap)
}
let target = NSPoint(x: shown.frame.midX, y: shown.frame.midY)
mouse(.mouseMoved, target)
pump(0.15)
expect(cancel.image != nil, "hover must reveal cancellation")
mouse(.mouseMoved, originalPointer)
pump(0.15)
expect(cancel.image == nil, "leaving the indicator should restore the orb")
mouse(.mouseMoved, target)
pump(0.15)
mouse(.leftMouseDown, target)
mouse(.leftMouseUp, target)
pump(0.15)
mouse(.mouseMoved, originalPointer)
expect(progressTakeCancelled(handle), "native cancel must reach Rust")
expect(!progressTakeCancelled(handle), "cancel must be consumed once")
expect(visiblePanel() == nil)
show(handle)
pump(0.4)
progressSucceed(handle)
pump(0.9)
expect(visiblePanel() == nil, "verified success must dismiss")
show(handle)
pump(0.4)
progressSucceed(handle)
pump(0.6)
show(handle, "New command")
pump(0.45)
expect(visiblePanel() != nil, "old success callbacks hid a new command")
progressHide(handle)
pump(0.3)
expect(visiblePanel() == nil)
show(handle)
progressDestroy(handle)
pump(0.4)
expect(visiblePanel() == nil, "destroy must invalidate pending display")
let other = progressCreate()
show(other)
pump(0.23)
progressDestroy(other)
pump(0.4)
expect(visiblePanel() == nil, "destroy during fade must dismiss safely")
expect(notActivated(), "animation changed foreground app")

// MARK: Ink Sweep lifecycle
/// Visible windows with the given title.
func windows(_ title: String) -> [NSWindow] { app.windows.filter { $0.title == title && $0.isVisible } }
let visibleArea = NSScreen.screens[0].visibleFrame
let sampleLines = [
    NSRect(x: visibleArea.midX - 260, y: visibleArea.midY + 20, width: 520, height: 18),
    NSRect(x: visibleArea.midX - 260, y: visibleArea.midY + 2, width: 300, height: 18),
]
let sampleElement = NSRect(x: visibleArea.midX - 300, y: visibleArea.midY - 200, width: 600, height: 400)
let sweepHandle = progressCreate()
let sweepController = progressController(sweepHandle)
sweepController.injectCapture(lines: sampleLines, element: sampleElement)
show(sweepHandle, "Concise")
progressSucceed(sweepHandle)
pump(0.4)
expect(windows("Selara Sweep").isEmpty && windows("Selara Chip").isEmpty && windows("Selara Afterglow").isEmpty,
       "fast sweep commands must not flash")
show(sweepHandle, "Concise")
pump(0.1)
expect(windows("Selara Chip").isEmpty, "sweep waits for the fast-command delay")
pump(0.3)
let highlight = windows("Selara Sweep")
let chipWindow = windows("Selara Chip").first
expect(highlight.count == 1, "one highlight panel per display")
expect(chipWindow != nil, "chip names the command")
expect(visiblePanel() == nil, "the orb is not shown when line bounds are plausible")
expect(highlight.allSatisfy { $0.ignoresMouseEvents && !$0.canBecomeKey && !$0.isOpaque }, "highlight is click-through")
expect(chipWindow.map { !$0.canBecomeKey && !$0.canBecomeMain && !$0.ignoresMouseEvents } == true,
       "chip takes clicks but never focus")
expect(chipWindow.map { $0.frame.minY >= sampleLines[0].maxY } == true, "chip sits above the first line")
expect(highlight.first.map { $0.frame.contains(sampleLines[0]) && $0.frame.contains(sampleLines[1]) } == true,
       "highlight covers every selected line")
expect(notActivated(), "sweep stole focus")
let shimmering = highlight.first?.contentView?.layer?.sublayers?.contains { line in
    line.sublayers?.contains { $0.animation(forKey: "shimmer") != nil } == true
} == true
expect(shimmering, "the highlight shimmers while working")
let chipButton = findButton(chipWindow?.contentView)!
let chipCenter = NSPoint(x: chipWindow!.frame.midX, y: chipWindow!.frame.midY)
mouse(.mouseMoved, chipCenter)
pump(0.15)
mouse(.leftMouseDown, chipCenter)
mouse(.leftMouseUp, chipCenter)
pump(0.15)
mouse(.mouseMoved, originalPointer)
expect(chipButton.accessibilityLabel()?.contains("Cancel") == true, "chip cancel is accessible")
expect(progressTakeCancelled(sweepHandle), "clicking the chip cancels like the orb")
expect(windows("Selara Chip").isEmpty && windows("Selara Sweep").isEmpty, "cancel hides the sweep")
show(sweepHandle, "Concise")
pump(0.4)
progressSucceedRange(sweepHandle, -1, -1)
pump(0.2)
expect(windows("Selara Sweep").isEmpty && windows("Selara Chip").isEmpty, "success ends the working sweep")
expect(windows("Selara Afterglow").count == 1, "success shows the afterglow")
expect(windows("Selara Receipt").count == 1, "success shows the undo receipt")
show(sweepHandle, "Next")
pump(0.05)
expect(windows("Selara Afterglow").isEmpty && windows("Selara Receipt").isEmpty, "a new run clears old receipts")
pump(0.4)
expect(windows("Selara Chip").count == 1, "old afterglow callbacks must not hide a new run")
progressSucceedRange(sweepHandle, -1, -1)
pump(2.4)
expect(windows("Selara Afterglow").isEmpty && windows("Selara Receipt").isEmpty, "afterglow and receipt fade away")
Motion.forceReduceMotion = true
show(sweepHandle, "Concise")
pump(0.4)
let still = windows("Selara Sweep").first?.contentView?.layer?.sublayers?.contains { line in
    line.sublayers?.contains { $0.animation(forKey: "shimmer") != nil } == true
} == true
expect(!still && windows("Selara Sweep").count == 1, "Reduce Motion keeps a static tint without shimmer")
progressHide(sweepHandle)
Motion.forceReduceMotion = false
sweepController.injectCapture(lines: [NSRect(x: 100, y: 300, width: 0, height: 0)], element: nil)
show(sweepHandle, "Concise")
pump(0.4)
expect(visiblePanel() != nil && windows("Selara Sweep").isEmpty, "implausible bounds fall back to the orb")
progressHide(sweepHandle)

// MARK: Ghost Diff review card
sweepController.injectCapture(lines: sampleLines, element: sampleElement)
let savedPasteboard = NSPasteboard.general.string(forType: .string)
/// Sends a key press to a window the way AppKit would.
func press(_ window: NSWindow, _ keyCode: UInt16, _ characters: String, _ flags: NSEvent.ModifierFlags = []) {
    let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
                                 windowNumber: window.windowNumber, context: nil, characters: characters,
                                 charactersIgnoringModifiers: characters, isARepeat: false, keyCode: keyCode)!
    if flags.contains(.command) { _ = window.performKeyEquivalent(with: event) } else { window.sendEvent(event) }
}
func showReview(_ title: String = "Concise", _ model: String = "gpt-5.4-mini") {
    title.withCString { t in model.withCString { m in progressShowReview(sweepHandle, t, m) } }
}
func reviewResult(_ json: String) -> Bool { json.withCString { progressReviewResult(sweepHandle, $0) } }
let reviewJSON = #"{"segments":[{"op":"delete","text":"I just wanted to check whether you had"},{"op":"insert","text":"Did you get"},{"op":"equal","text":" a chance to review it"},{"op":"delete","text":"."},{"op":"insert","text":"?"}],"delta":-14,"result":"Did you get a chance to review it?"}"#
showReview()
pump(0.1)
expect(windows("Selara Review").isEmpty, "the card waits for the fast-command delay")
pump(0.3)
let card = windows("Selara Review").first
expect(card != nil, "review commands show the card")
expect(windows("Selara Sweep").isEmpty && visiblePanel() == nil, "review replaces the sweep and orb")
expect(card.map { !$0.isKeyWindow } == true, "the skeleton never takes focus")
expect(card.map { $0.frame.maxY <= sampleLines[1].minY } == true, "card sits below the selection")
expect(card.map { abs($0.frame.width - 470) < 0.5 } == true, "card is 470 pt wide")
expect(progressTakeReviewAction(sweepHandle) == 0, "no action yet")
press(card!, 36, "\r")
expect(progressTakeReviewAction(sweepHandle) == 0, "↩ before the result does nothing")
expect(reviewResult(reviewJSON), "result renders as a diff")
pump(0.1)
expect(card!.isKeyWindow, "the diff card becomes key to receive ↩ ⇥ ⌘C esc")
expect(notActivated(), "the key card must not activate Selara")
press(card!, 48, "\t")
expect(progressTakeReviewAction(sweepHandle) == 2, "⇥ asks for another take")
expect(card!.isVisible && !card!.isKeyWindow, "another take keeps the card without focus")
showReview()
pump(0.05)
expect(sweepController.card.state == .skeleton && card!.isVisible, "another take shows the skeleton immediately")
expect(reviewResult(reviewJSON))
pump(0.1)
press(card!, 8, "c", .command)
expect(progressTakeReviewAction(sweepHandle) == 3, "⌘C copies")
expect(NSPasteboard.general.string(forType: .string) == "Did you get a chance to review it?", "⌘C puts the result on the pasteboard")
progressCloseReview(sweepHandle, true)
pump(0.3)
expect(windows("Selara Review").isEmpty, "closing hides the card")
expect(windows("Selara Receipt").count == 1, "copy leaves a Copied receipt")
showReview()
pump(0.4)
expect(reviewResult(reviewJSON))
pump(0.1)
press(windows("Selara Review").first!, 53, "\u{1b}")
expect(progressTakeReviewAction(sweepHandle) == 4, "esc discards")
progressCloseReview(sweepHandle, false)
pump(0.3)
showReview()
pump(0.4)
expect(reviewResult(reviewJSON))
pump(0.1)
windows("Selara Review").first!.resignKey()
expect(progressTakeReviewAction(sweepHandle) == 5, "losing focus dismisses the card")
progressCloseReview(sweepHandle, false)
pump(0.3)
showReview()
pump(0.4)
expect(reviewResult(reviewJSON))
pump(0.1)
press(windows("Selara Review").first!, 36, "\r")
expect(progressTakeReviewAction(sweepHandle) == 1, "↩ accepts")
pump(0.05)
expect(windows("Selara Review").isEmpty, "accept hands focus back by hiding the card at once")
progressSucceedRange(sweepHandle, -1, -1)
pump(0.2)
expect(windows("Selara Afterglow").count == 1 && windows("Selara Receipt").count == 1,
       "accepted review ends with the afterglow and undo receipt")
progressHide(sweepHandle)
sweepController.injectCapture(lines: [], element: nil)
showReview()
pump(0.4)
expect(windows("Selara Review").count == 1, "the card still anchors without line bounds")
progressHide(sweepHandle)
pump(0.1)
expect(windows("Selara Review").isEmpty, "hide closes the card")
expect(!reviewResult(reviewJSON), "a closed card ignores late results")
progressDestroy(sweepHandle)
if let savedPasteboard {
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(savedPasteboard, forType: .string)
}
expect(notActivated(), "overlays changed the foreground app")
print("PASS: native progress size, focus, fast completion, cancellation, success, overlapping runs, and destruction")
print("PASS: ink sweep, orb fallback, reduce motion, review card keys, focus, copy, and dismissal")
