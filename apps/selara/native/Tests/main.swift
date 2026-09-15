// Native geometry and lifecycle regressions: compile with Progress.swift and the orb sources.
import AppKit

// Swift assertions disappear under -O; keep checks active with readable failures.
func expect(_ condition: @autoclosure () -> Bool, _ message: String = "", line: UInt = #line) {
    guard condition() else {
        fputs("FAIL at line \(line): \(message)\n", stderr)
        exit(1)
    }
}

// Golden samples captured from thinking-orbs v0.3.1's JavaScript working64
// preset (bd204b73c9b6660fad7210b1ad48d9dc2adbb89d), retained from PR #119.
// Exercise the shipped Swift engine and preset, including its clock multiplier.
// Use the upstream Swift port's 1e-4 tolerance for cross-platform trig differences.
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

checkWorkingOrbGeometry()
if CommandLine.arguments.contains("--geometry-only") { exit(0) }

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let sourcePID = NSWorkspace.shared.frontmostApplication?.processIdentifier
func pump(_ seconds: TimeInterval) {
    let until = Date().addingTimeInterval(seconds)
    while Date() < until {
        while let event = app.nextEvent(matching: .any, until: Date(), inMode: .default, dequeue: true) {
            app.sendEvent(event)
        }
        RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    }
}
func visiblePanel() -> NSWindow? { app.windows.first { $0.title == "Selara" && $0.isVisible } }
func show(_ handle: UnsafeMutableRawPointer, _ title: String = "RewritePro") {
    title.withCString { progressShow(handle, $0) }
}
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
expect(NSWorkspace.shared.frontmostApplication?.processIdentifier == sourcePID, "progress stole focus")
let cancel = findButton(shown.contentView)!
expect(cancel.image == nil, "cancel affordance should be quiet until hovered")
let originalPointer = NSEvent.mouseLocation
let primaryTop = NSScreen.screens[0].frame.maxY
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
expect(NSWorkspace.shared.frontmostApplication?.processIdentifier == sourcePID, "animation changed foreground app")
print("PASS: native progress size, focus, fast completion, cancellation, success, overlapping runs, and destruction")
