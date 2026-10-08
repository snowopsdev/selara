// Moment-of-use preview: renders the production Ink Sweep, orb fallback, and
// Ghost Diff card (apps/selara/native/*.swift) over a real NSTextView with a
// selected paragraph. No provider, clipboard, or document access.
//
//   sh examples/run-native-moment-preview.sh --mode sweep-working --appearance dark
//   sh examples/run-native-moment-preview.sh --mode ghost-diff --capture /tmp/card.png
//
// Modes: sweep-working, sweep-done, orb-fallback, ghost-skeleton, ghost-diff, cycle.
// Line bounds come from the text view's layout, the same rectangles
// Accessibility reports through AXBoundsForRange in AppKit text views.
import AppKit

struct Options {
    var mode = "cycle"
    var dark = false
    var reduceMotion = false
    var capture: String?

    init(_ arguments: [String]) {
        var iterator = arguments.dropFirst().makeIterator()
        while let argument = iterator.next() {
            switch argument {
            case "--mode": mode = iterator.next() ?? mode
            case "--appearance": dark = iterator.next() == "dark"
            case "--reduce-motion": reduceMotion = true
            case "--capture": capture = iterator.next()
            default: break
            }
        }
    }
}

let original = "I just wanted to quickly follow up and check in on whether you had a chance to take a look at the document I sent over last week."
let rewritten = "Did you get a chance to review the document I sent last week?"
// Output of `review::diff_payload(original, rewritten)` (see review.rs tests).
let diffJSON = #"{"segments":[{"op":"delete","text":"I just wanted to quickly follow up and check in on whether you had"},{"op":"insert","text":"Did you get"},{"op":"equal","text":" a chance to "},{"op":"delete","text":"take a look at"},{"op":"insert","text":"review"},{"op":"equal","text":" the document I sent "},{"op":"delete","text":"over "},{"op":"equal","text":"last week"},{"op":"delete","text":"."},{"op":"insert","text":"?"}],"delta":-68,"result":"Did you get a chance to review the document I sent last week?"}"#

final class Preview: NSObject, NSApplicationDelegate {
    let options = Options(CommandLine.arguments)
    var window: NSWindow!
    var textView: NSTextView!
    let controller = ProgressController()
    var selection = NSRange(location: 0, length: 0)

    func applicationDidFinishLaunching(_ notification: Notification) {
        Motion.forceReduceMotion = options.reduceMotion
        let appearance = NSAppearance(named: options.dark ? .darkAqua : .aqua)
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 620, height: 430),
                          styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "Launch doc"
        window.appearance = appearance
        window.isReleasedWhenClosed = false
        // Stay above other apps' windows so captures show only this preview.
        window.level = .floating
        window.center()
        let content = window.contentView!
        let header = NSTextField(labelWithString: "To: Dana Whitfield")
        header.font = .systemFont(ofSize: 12.5)
        header.textColor = .secondaryLabelColor
        header.frame = NSRect(x: 20, y: 392, width: 580, height: 18)
        content.addSubview(header)
        let rule = NSBox(frame: NSRect(x: 20, y: 382, width: 580, height: 1))
        rule.boxType = .separator
        content.addSubview(rule)
        textView = NSTextView(frame: NSRect(x: 16, y: 20, width: 470, height: 352))
        textView.textContainerInset = NSSize(width: 4, height: 6)
        textView.isEditable = true
        textView.drawsBackground = false
        let paragraph = NSMutableParagraphStyle()
        paragraph.minimumLineHeight = 22
        paragraph.maximumLineHeight = 22
        let body = "Hi Dana,\n\n\(original)\n\nThanks,\nSam"
        textView.textStorage?.setAttributedString(NSAttributedString(string: body, attributes: [
            .font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor.labelColor, .paragraphStyle: paragraph,
        ]))
        textView.typingAttributes = [.font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor.labelColor,
                                     .paragraphStyle: paragraph]
        content.addSubview(textView)
        selection = NSRange(location: ("Hi Dana,\n\n" as NSString).length, length: (original as NSString).length)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        window.makeFirstResponder(textView)
        textView.setSelectedRange(selection)
        controller.appearanceOverride = appearance
        controller.replacedLinesProvider = { [weak self] _, _, _ in
            guard let self else { return nil }
            return self.lineRects(NSRange(location: self.selection.location, length: (rewritten as NSString).length))
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { self.run(self.options.mode) }
    }

    /// Screen rects of each laid-out line of `range`, top line first.
    func lineRects(_ range: NSRange) -> [NSRect] {
        guard let layout = textView.layoutManager, let container = textView.textContainer else { return [] }
        let glyphs = layout.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
        var rects: [NSRect] = []
        layout.enumerateLineFragments(forGlyphRange: glyphs) { _, used, _, lineGlyphs, _ in
            let part = NSIntersectionRange(lineGlyphs, glyphs)
            guard part.length > 0 else { return }
            var rect = layout.boundingRect(forGlyphRange: part, in: container)
            rect.size.height = used.height
            rect.origin.y = used.minY
            rect = rect.offsetBy(dx: self.textView.textContainerOrigin.x, dy: self.textView.textContainerOrigin.y)
            let inWindow = self.textView.convert(rect, to: nil)
            rects.append(self.window.convertToScreen(inWindow))
        }
        return rects
    }

    var editorFrame: NSRect { window.convertToScreen(textView.convert(textView.bounds, to: nil)) }

    func run(_ mode: String) {
        let lines = lineRects(selection)
        switch mode {
        case "sweep-working":
            controller.injectCapture(lines: lines, element: editorFrame)
            controller.show("Concise")
            finish(after: 0.18 + 0.55)
        case "sweep-done":
            controller.injectCapture(lines: lines, element: editorFrame)
            controller.show("Concise")
            after(0.6) { self.replaceSelection(); self.controller.succeed(location: 0, length: 1) }
            finish(after: 0.6 + 0.5)
        case "orb-fallback":
            // A Chromium/Electron-style misreport: a zero-size rect at the
            // selection start. Bounds are implausible, so the orb is used.
            controller.injectCapture(lines: [NSRect(x: lines[0].minX, y: lines[0].minY, width: 0, height: 0)],
                                     element: editorFrame)
            controller.show("Concise")
            finish(after: 0.18 + 0.6)
        case "ghost-skeleton":
            controller.injectCapture(lines: lines, element: editorFrame)
            controller.showReview("Concise", model: "gpt-5.4-mini")
            finish(after: 0.18 + 0.5)
        case "ghost-diff":
            controller.injectCapture(lines: lines, element: editorFrame)
            controller.showReview("Concise", model: "gpt-5.4-mini")
            after(0.5) { _ = self.controller.reviewResult(diffJSON) }
            finish(after: 0.5 + 0.6)
        default:
            cycle(["sweep-working", "sweep-done", "orb-fallback", "ghost-skeleton", "ghost-diff"], index: 0)
        }
    }

    func cycle(_ modes: [String], index: Int) {
        resetText()
        run(modes[index % modes.count])
        after(3.2) {
            self.controller.hide()
            self.cycle(modes, index: index + 1)
        }
    }

    func resetText() {
        let body = "Hi Dana,\n\n\(original)\n\nThanks,\nSam"
        if textView.string != body {
            textView.textStorage?.replaceCharacters(in: NSRange(location: 0, length: (textView.string as NSString).length),
                                                    with: body)
        }
        window.makeKeyAndOrderFront(nil)
        textView.setSelectedRange(selection)
    }

    func replaceSelection() {
        textView.insertText(rewritten, replacementRange: selection)
    }

    func after(_ delay: TimeInterval, _ action: @escaping () -> Void) {
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: action)
    }

    /// With `--capture`, screenshot the window region (overlays included) and exit.
    func finish(after delay: TimeInterval) {
        guard let path = options.capture else { return }
        after(delay) {
            // Include the editor gutters, where the orb fallback sits.
            let frame = self.window.frame.insetBy(dx: -72, dy: -12)
            let top = NSScreen.screens[0].frame.maxY
            let region = "\(Int(frame.minX)),\(Int(top - frame.maxY)),\(Int(frame.width)),\(Int(frame.height))"
            let task = Process()
            task.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            task.arguments = ["-x", "-R", region, path]
            try? task.run()
            task.waitUntilExit()
            print("captured \(self.options.mode) → \(path)")
            NSApp.terminate(nil)
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let preview = Preview()
app.delegate = preview
app.run()
