// Standalone preview of the production custom-instruction popover
// (apps/selara/native/Instruction*.swift). No provider, clipboard, or document
// access: events are printed instead of sent to Rust.
// Run: sh examples/run-native-instruction-preview.sh [--dark] [--state off|chips|detail|recall|saved|above]
import AppKit

let arguments = CommandLine.arguments
func option(_ name: String) -> String? {
    guard let index = arguments.firstIndex(of: name), index + 1 < arguments.count else { return nil }
    return arguments[index + 1]
}
let dark = arguments.contains("--dark")
let state = option("--state") ?? "chips"

final class Preview: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    let controller = InstructionPopoverController()
    let document = NSTextView(frame: .zero)
    let greeting = "Hi Dana,\n\n"
    let sentence = "I just wanted to quickly follow up and check in on whether you had a chance to take a look at the document I sent over last week."
    var timer: Timer?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 640, height: 420),
                          styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "Selara · Custom instruction preview"
        window.isReleasedWhenClosed = false
        window.center()
        // Leave room under the paragraph for the popover, as in the reference.
        let top = state == "above" ? 300.0 : 40.0
        document.frame = NSRect(x: 0, y: 0, width: 640, height: 420)
        document.textContainerInset = NSSize(width: 26, height: top)
        document.isVerticallyResizable = false
        document.isEditable = false
        document.isSelectable = false
        document.font = .systemFont(ofSize: 14)
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = 5
        let base: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor.labelColor, .paragraphStyle: paragraph]
        let text = NSMutableAttributedString(string: greeting, attributes: base)
        var selected = base
        selected[.backgroundColor] = NSColor.selectedTextBackgroundColor
        text.append(NSAttributedString(string: sentence, attributes: selected))
        document.textStorage?.setAttributedString(text)
        window.contentView = document
        if state == "above", let visible = window.screen?.visibleFrame {
            // No room under the selection: the popover must flip above it.
            window.setFrameOrigin(NSPoint(x: window.frame.minX, y: visible.minY))
        }
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { self.present() }
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            while let event = self?.controller.takeEvent() {
                print("event:", event.encoded.replacingOccurrences(of: "\n", with: " ⏎ "))
                if case .save = event { self?.controller.showNotice("Saved as command “Make it shorter and”", success: true) }
                if case .submit = event { DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { self?.present() } }
                if case .cancel = event { DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { self?.present() } }
            }
        }
    }

    /// The highlighted sentence's bounds in screen points: what AX would report.
    func selectionRect() -> NSRect {
        guard let layout = document.layoutManager, let container = document.textContainer else { return .zero }
        let range = NSRange(location: (greeting as NSString).length, length: (sentence as NSString).length)
        let glyphs = layout.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
        var rect = layout.boundingRect(forGlyphRange: glyphs, in: container)
        rect.origin.x += document.textContainerOrigin.x
        rect.origin.y += document.textContainerOrigin.y
        return window.convertToScreen(document.convert(rect, to: nil))
    }

    func present() {
        controller.present(app: "Mail", chars: UInt64(sentence.count),
                           history: ["Turn this into 3 short bullet points", "Make it sound less apologetic", "Translate to Spanish"],
                           anchor: selectionRect())
        switch state {
        case "chips", "above":
            controller.toggle(.shorter); controller.toggle(.warmer)
        case "detail":
            controller.toggle(.shorter); controller.toggle(.warmer)
            controller.setDetail("Keep the mention of last week.")
        case "recall":
            controller.recall(1)
        case "saved":
            controller.toggle(.shorter); controller.toggle(.direct)
            controller.save()
        default:
            break
        }
        if let path = ProcessInfo.processInfo.environment["SELARA_PREVIEW_INFO"] {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                let doc = self.window.frame, pop = self.controller.panel.frame
                let union = doc.union(pop)
                let top = NSScreen.screens[0].frame.maxY
                let info = "\(self.controller.panel.windowNumber) \(self.window.windowNumber) " +
                    "\(Int(union.minX)),\(Int(top - union.maxY)),\(Int(union.width)),\(Int(union.height))"
                try? info.write(toFile: path, atomically: true, encoding: .utf8)
            }
        }
    }
}

setvbuf(stdout, nil, _IOLBF, 0)
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = Preview()
app.delegate = delegate
app.run()
