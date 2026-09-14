// A standalone AppKit design preview. No provider, clipboard, or document access.
// Run: sh examples/run-native-progress-preview.sh
import AppKit
import SwiftUI
import ThinkingOrbsKit

final class OrbPlayback: ObservableObject {
    @Published var paused = true
}

struct PreviewOrb: View {
    @ObservedObject var playback: OrbPlayback
    var body: some View {
        // Match the production orb: dark particles and a white halo keep it
        // visible on either background without changing the chosen artwork.
        ThinkingOrb(state: .working, size: .px64,
                    theme: .light, paused: playback.paused, displaySize: 40)
            .shadow(color: .white.opacity(0.9), radius: 0.7)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}

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
        let area = NSTrackingArea(rect: bounds,
                                  options: [.activeAlways, .inVisibleRect, .mouseEnteredAndExited],
                                  owner: self)
        addTrackingArea(area)
        tracking = area
    }
    override func mouseEntered(with event: NSEvent) { hovered = true }
    override func mouseExited(with event: NSEvent) { hovered = false }
}

final class ProgressOrbPanel: NSPanel {
    let playback = OrbPlayback()
    var orb: NSHostingView<PreviewOrb>!
    let done = NSImageView()
    private let cancelButton = OrbCancelButton()
    var onCancel: (() -> Void)?
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    init() {
        let side: CGFloat = 48
        super.init(contentRect: NSRect(x: 0, y: 0, width: side, height: side),
                   styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        isReleasedWhenClosed = false
        hidesOnDeactivate = false
        level = .floating
        collectionBehavior = [.fullScreenAuxiliary, .moveToActiveSpace]
        animationBehavior = .none
        acceptsMouseMovedEvents = true
        let content = NSView(frame: NSRect(x: 0, y: 0, width: side, height: side))
        content.wantsLayer = true
        content.layer?.backgroundColor = NSColor.clear.cgColor
        contentView = content
        orb = NSHostingView(rootView: PreviewOrb(playback: playback))
        let indicatorFrame = NSRect(x: 4, y: 4, width: 40, height: 40)
        orb.frame = indicatorFrame
        content.addSubview(orb)
        done.frame = NSRect(x: 14, y: 14, width: 20, height: 20)
        done.image = NSImage(systemSymbolName: "checkmark", accessibilityDescription: "Updated")
        done.contentTintColor = .systemGreen
        done.setAccessibilityLabel("Updated")
        done.isHidden = true
        content.addSubview(done)
        // Keep the control in the accessibility tree even while the orb is
        // shown. Hover only supplies its image, so the hit target stays in
        // the same 48-point panel without adding another visible element.
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
        content.addSubview(cancelButton)
        cancelButton.onHover = { [weak self] hovered in
            self?.orb.alphaValue = hovered ? 0 : 1
        }
    }

    func showWorking() {
        cancelButton.hovered = false
        orb.isHidden = false
        orb.alphaValue = 1
        cancelButton.isHidden = false
        done.isHidden = true
    }

    func showSuccess() {
        playback.paused = true
        orb.isHidden = true
        cancelButton.hovered = false
        cancelButton.isHidden = true
        done.isHidden = false
    }

    @objc func cancelRun() { onCancel?() }
}

final class Demo: NSObject, NSApplicationDelegate, NSWindowDelegate {
    var window: NSWindow!
    let capsule = ProgressOrbPanel()
    let paragraph = NSTextField(wrappingLabelWithString: "")
    let replay = NSButton(title: "Replay animation", target: nil, action: nil)
    let appearanceButton = NSButton(checkboxWithTitle: "Dark background", target: nil, action: nil)
    let motionButton = NSButton(checkboxWithTitle: "Reduce motion", target: nil, action: nil)
    var generation = 0
    var escapeMonitor: Any?
    var reducedMotion: Bool { motionButton.state == .on || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }
    let original = "I am writing to inform you that due to the fact that the meeting has been postponed, we will need to make an adjustment to our schedule."
    let rewritten = "The meeting has been postponed, so we need to adjust our schedule."

    func label(_ text: String, frame: NSRect, font: NSFont, color: NSColor = .labelColor) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.frame = frame; label.font = font; label.textColor = color
        window.contentView!.addSubview(label)
        return label
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 760, height: 480), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "Selara · Command animation preview"
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.center()
        window.backgroundColor = .windowBackgroundColor
        _ = label("A quieter way to rewrite", frame: NSRect(x: 36, y: 397, width: 680, height: 34), font: .systemFont(ofSize: 24, weight: .semibold))
        _ = label("Native preview · simulated command · your documents stay untouched", frame: NSRect(x: 36, y: 371, width: 688, height: 20), font: .systemFont(ofSize: 12), color: .secondaryLabelColor)
        let paper = NSBox(frame: NSRect(x: 36, y: 175, width: 688, height: 172))
        paper.boxType = .custom
        paper.fillColor = .textBackgroundColor
        paper.borderColor = .separatorColor
        paper.borderWidth = 0.5
        paper.cornerRadius = 12
        window.contentView!.addSubview(paper)
        _ = label("DRAFT", frame: NSRect(x: 58, y: 305, width: 600, height: 16), font: .systemFont(ofSize: 10, weight: .medium), color: .secondaryLabelColor)
        paragraph.frame = NSRect(x: 58, y: 208, width: 620, height: 79)
        paragraph.font = .systemFont(ofSize: 17)
        paragraph.maximumNumberOfLines = 4
        paragraph.stringValue = original
        window.contentView!.addSubview(paragraph)
        replay.frame = NSRect(x: 36, y: 26, width: 148, height: 32)
        replay.bezelStyle = .rounded
        replay.target = self; replay.action = #selector(start)
        window.contentView!.addSubview(replay)
        appearanceButton.state = NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? .on : .off
        appearanceButton.frame = NSRect(x: 366, y: 30, width: 166, height: 24)
        appearanceButton.target = self; appearanceButton.action = #selector(changeAppearance)
        window.contentView!.addSubview(appearanceButton)
        motionButton.frame = NSRect(x: 556, y: 30, width: 160, height: 24)
        window.contentView!.addSubview(motionButton)
        capsule.onCancel = { [weak self] in self?.cancel() }
        escapeMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            if event.keyCode == 53 { self?.cancel(); return nil }
            return event
        }
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        window.addChildWindow(capsule, ordered: .above)
        positionCapsule()
        capsule.orderOut(nil)
        // Expose only this synthetic preview's capture rectangle.
        if let path = ProcessInfo.processInfo.environment["SELARA_PREVIEW_RECT"] {
            let frame = window.frame
            let top = NSScreen.screens[0].frame.maxY
            try? "\(Int(frame.minX)),\(Int(top - frame.maxY)),\(Int(frame.width)),\(Int(frame.height))".write(toFile: path, atomically: true, encoding: .utf8)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { self.start() }
    }
    func positionCapsule() {
        // Keep the indicator in the editor gutter, beside the sample text
        // rather than over the paragraph or beneath the draft.
        let paragraphRect = paragraph.convert(paragraph.bounds, to: window.contentView)
        let screenRect = window.convertToScreen(paragraphRect)
        let gap: CGFloat = 8
        capsule.setFrameOrigin(NSPoint(x: screenRect.minX - capsule.frame.width - gap,
                                       y: screenRect.midY - capsule.frame.height / 2))
    }
    func windowDidMove(_ notification: Notification) { positionCapsule() }
    @objc func changeAppearance() {
        let appearance = NSAppearance(named: appearanceButton.state == .on ? .darkAqua : .aqua)
        window.appearance = appearance
        capsule.appearance = appearance
    }
    @objc func start() {
        generation += 1
        let current = generation
        paragraph.stringValue = original
        replay.isEnabled = false
        capsule.showWorking()
        capsule.playback.paused = reducedMotion
        capsule.alphaValue = reducedMotion ? 1 : 0
        positionCapsule()
        let frontmost = NSWorkspace.shared.frontmostApplication?.processIdentifier
        capsule.orderFrontRegardless()
        NSAnimationContext.runAnimationGroup { context in
            context.duration = reducedMotion ? 0 : 0.18
            capsule.animator().alphaValue = 1
        }
        print("Preview shown; foreground preserved: \(frontmost == NSWorkspace.shared.frontmostApplication?.processIdentifier); panel can become key: \(capsule.canBecomeKey)")
        DispatchQueue.main.asyncAfter(deadline: .now() + 4.0) {
            guard current == self.generation else { return }
            self.paragraph.stringValue = self.rewritten
            self.capsule.showSuccess()
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.65) {
                guard current == self.generation else { return }
                self.dismiss(current)
            }
        }
    }
    func dismiss(_ current: Int) {
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = reducedMotion ? 0 : 0.2
            capsule.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            guard let self, current == self.generation else { return }
            self.capsule.orderOut(nil)
            self.replay.isEnabled = true
        })
    }
    func cancel() {
        generation += 1
        capsule.playback.paused = true
        dismiss(generation)
    }
    func windowWillClose(_ notification: Notification) { NSApp.terminate(nil) }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let demo = Demo()
app.delegate = demo
app.run()
