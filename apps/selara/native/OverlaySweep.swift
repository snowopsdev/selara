// 1A Ink Sweep: the selection itself is the progress indicator. Click-through
// highlight panels sit over each selected line with an accent tint and a
// moving specular band; a small chip above the first line names the command
// and cancels on click. Success wipes a green afterglow over the replaced
// range and shows a “Replaced · ⌘Z to undo” receipt. Main thread only.
import AppKit
import QuartzCore

/// Borderless, non-activating overlay window. It never becomes key or main,
/// so the source app keeps focus and its selection.
final class OverlayPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    init(title: String, clickThrough: Bool) {
        super.init(contentRect: NSRect(x: 0, y: 0, width: 10, height: 10),
                   styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        self.title = title
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        isReleasedWhenClosed = false
        hidesOnDeactivate = false
        level = .floating
        collectionBehavior = [.fullScreenAuxiliary, .moveToActiveSpace, .ignoresCycle, .transient]
        animationBehavior = .none
        ignoresMouseEvents = clickThrough
        acceptsMouseMovedEvents = !clickThrough
        let content = NSView(frame: NSRect(x: 0, y: 0, width: 10, height: 10))
        content.wantsLayer = true
        content.layer?.backgroundColor = NSColor.clear.cgColor
        contentView = content
    }
}

/// Colors for the overlays. Chips invert against the system appearance
/// (dark ink on light, light ink on dark) so they read over any document.
struct OverlayPalette {
    let appearance: NSAppearance
    var dark: Bool { appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua }

    static func resolve(_ appearance: NSAppearance?) -> OverlayPalette {
        OverlayPalette(appearance: appearance ?? NSApp.effectiveAppearance)
    }

    func cg(_ color: NSColor) -> CGColor {
        var resolved = color.cgColor
        appearance.performAsCurrentDrawingAppearance { resolved = color.cgColor }
        return resolved
    }

    var chipFill: NSColor { dark ? NSColor(srgbRed: 0.949, green: 0.949, blue: 0.969, alpha: 1)
                                 : NSColor(srgbRed: 0.114, green: 0.114, blue: 0.122, alpha: 1) }
    var chipText: NSColor { dark ? NSColor(srgbRed: 0.114, green: 0.114, blue: 0.122, alpha: 1) : .white }
    var chipKeyFill: NSColor { dark ? NSColor.black.withAlphaComponent(0.08) : NSColor.white.withAlphaComponent(0.16) }
    var tint: CGColor { cg(NSColor.controlAccentColor.withAlphaComponent(dark ? 0.12 : 0.07)) }
    var shimmer: CGColor { NSColor.white.withAlphaComponent(dark ? 0.26 : 0.6).cgColor }
    var glow: CGColor { cg(NSColor.systemGreen.withAlphaComponent(dark ? 0.34 : 0.28)) }
}

/// A small rounded key label (⌘, Z, ↩, esc).
final class KeycapView: NSView {
    enum Style { case chip, card }
    private let label: NSTextField

    init(_ text: String, style: Style, palette: OverlayPalette) {
        label = NSTextField(labelWithString: text)
        super.init(frame: .zero)
        wantsLayer = true
        let fontSize: CGFloat = style == .card ? 11.5 : 10.5
        label.font = .systemFont(ofSize: fontSize, weight: .medium)
        label.alignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        let height: CGFloat = style == .card ? 20 : 16
        translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            heightAnchor.constraint(equalToConstant: height),
            widthAnchor.constraint(greaterThanOrEqualToConstant: height),
            label.centerYAnchor.constraint(equalTo: centerYAnchor, constant: style == .card ? 0 : 0.5),
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: style == .card ? 5 : 4),
            label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: style == .card ? -5 : -4),
        ])
        switch style {
        case .chip:
            label.textColor = palette.chipText
            layer?.cornerRadius = 4
            layer?.backgroundColor = palette.cg(palette.chipKeyFill)
        case .card:
            label.textColor = .labelColor
            layer?.cornerRadius = 5
            layer?.backgroundColor = palette.cg(palette.dark ? NSColor(white: 1, alpha: 0.08) : .white)
            layer?.borderWidth = 0.5
            layer?.borderColor = palette.cg(palette.dark ? NSColor(white: 1, alpha: 0.18) : NSColor(white: 0, alpha: 0.16))
            layer?.shadowColor = palette.cg(palette.dark ? NSColor(white: 1, alpha: 0.18) : NSColor(white: 0, alpha: 0.16))
            layer?.shadowOpacity = 1
            layer?.shadowRadius = 0
            layer?.shadowOffset = CGSize(width: 0, height: -1)
        }
        label.setAccessibilityElement(false)
    }

    required init?(coder: NSCoder) { fatalError("unused") }
}

func chipLabel(_ text: String, size: CGFloat = 11, weight: NSFont.Weight, color: NSColor) -> NSTextField {
    let label = NSTextField(labelWithString: text)
    label.font = .systemFont(ofSize: size, weight: weight)
    label.textColor = color
    label.lineBreakMode = .byTruncatingTail
    label.setContentCompressionResistancePriority(.required, for: .horizontal)
    return label
}

/// Rounded pill with a horizontal stack of labels and keycaps.
class ChipView: NSView {
    let stack = NSStackView()
    let palette: OverlayPalette

    init(palette: OverlayPalette, leading: CGFloat = 6, trailing: CGFloat = 8) {
        self.palette = palette
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.backgroundColor = palette.cg(palette.chipFill)
        layer?.shadowColor = NSColor.black.cgColor
        layer?.shadowOpacity = palette.dark ? 0.35 : 0.18
        layer?.shadowRadius = 4
        layer?.shadowOffset = CGSize(width: 0, height: -1)
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 3, left: leading, bottom: 3, right: trailing)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            heightAnchor.constraint(greaterThanOrEqualToConstant: 21),
        ])
    }

    required init?(coder: NSCoder) { fatalError("unused") }

    var fittingChipSize: NSSize {
        layoutSubtreeIfNeeded()
        let size = fittingSize
        return NSSize(width: ceil(size.width), height: max(21, ceil(size.height)))
    }
}

/// Mini conic orb that spins while the command runs.
final class MiniOrbView: NSView {
    private let gradient = CAGradientLayer()

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        gradient.type = .conic
        gradient.startPoint = CGPoint(x: 0.5, y: 0.5)
        gradient.endPoint = CGPoint(x: 0.5, y: 1)
        let hexes: [UInt32] = [0x0a84ff, 0xbf5af2, 0xff375f, 0x0a84ff]
        gradient.colors = hexes.map(MiniOrbView.cgColor(hex:))
        gradient.cornerRadius = 5
        gradient.masksToBounds = true
        layer?.addSublayer(gradient)
        translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([widthAnchor.constraint(equalToConstant: 10),
                                     heightAnchor.constraint(equalToConstant: 10)])
        setAccessibilityElement(false)
    }

    required init?(coder: NSCoder) { fatalError("unused") }

    // Spelled out step by step: the one-expression form exceeded the type
    // checker's time limit on CI's Swift toolchain.
    private static func cgColor(hex: UInt32) -> CGColor {
        let red: CGFloat = CGFloat((hex >> 16) & 0xff) / 255
        let green: CGFloat = CGFloat((hex >> 8) & 0xff) / 255
        let blue: CGFloat = CGFloat(hex & 0xff) / 255
        return NSColor(srgbRed: red, green: green, blue: blue, alpha: 1).cgColor
    }

    override func layout() {
        super.layout()
        gradient.frame = bounds
    }

    func spin(_ on: Bool) {
        gradient.removeAnimation(forKey: "spin")
        guard on else { return }
        let rotation = CABasicAnimation(keyPath: "transform.rotation.z")
        rotation.fromValue = 0
        rotation.toValue = -2 * Double.pi
        rotation.duration = 1
        rotation.repeatCount = .infinity
        gradient.add(rotation, forKey: "spin")
    }
}

/// The working chip: mini orb, command name, “esc to cancel”. Hovering swaps
/// the orb for a cancel glyph; clicking cancels like the orb does today.
final class SweepChipView: ChipView {
    let orb = MiniOrbView(frame: NSRect(x: 0, y: 0, width: 10, height: 10))
    let cancelGlyph = NSImageView()
    let title: NSTextField
    let hint: NSTextField
    let button = NSButton()
    var onCancel: (() -> Void)?
    private var tracking: NSTrackingArea?

    init(command: String, palette: OverlayPalette) {
        title = chipLabel(command, weight: .semibold, color: palette.chipText)
        hint = chipLabel("esc to cancel", weight: .medium, color: palette.chipText.withAlphaComponent(0.6))
        super.init(palette: palette)
        cancelGlyph.image = NSImage(systemSymbolName: "xmark.circle.fill", accessibilityDescription: nil)
        cancelGlyph.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 10, weight: .semibold)
        cancelGlyph.contentTintColor = palette.chipText
        cancelGlyph.isHidden = true
        cancelGlyph.translatesAutoresizingMaskIntoConstraints = false
        let glyphSlot = NSView()
        glyphSlot.translatesAutoresizingMaskIntoConstraints = false
        glyphSlot.addSubview(orb)
        glyphSlot.addSubview(cancelGlyph)
        NSLayoutConstraint.activate([
            glyphSlot.widthAnchor.constraint(equalToConstant: 10), glyphSlot.heightAnchor.constraint(equalToConstant: 10),
            orb.centerXAnchor.constraint(equalTo: glyphSlot.centerXAnchor), orb.centerYAnchor.constraint(equalTo: glyphSlot.centerYAnchor),
            cancelGlyph.centerXAnchor.constraint(equalTo: glyphSlot.centerXAnchor), cancelGlyph.centerYAnchor.constraint(equalTo: glyphSlot.centerYAnchor),
        ])
        stack.addArrangedSubview(glyphSlot)
        stack.addArrangedSubview(title)
        stack.addArrangedSubview(hint)
        title.setAccessibilityElement(false)
        hint.setAccessibilityElement(false)
        button.title = ""
        button.isBordered = false
        button.isTransparent = true
        button.toolTip = "Cancel command (Esc)"
        button.setAccessibilityLabel("\(command) running. Cancel command")
        button.target = self
        button.action = #selector(cancel)
        button.translatesAutoresizingMaskIntoConstraints = false
        addSubview(button)
        NSLayoutConstraint.activate([
            button.leadingAnchor.constraint(equalTo: leadingAnchor), button.trailingAnchor.constraint(equalTo: trailingAnchor),
            button.topAnchor.constraint(equalTo: topAnchor), button.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("unused") }

    var hovered = false {
        didSet {
            orb.isHidden = hovered
            cancelGlyph.isHidden = !hovered
            hint.textColor = palette.chipText.withAlphaComponent(hovered ? 0.95 : 0.6)
        }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds, options: [.activeAlways, .inVisibleRect, .mouseEnteredAndExited],
                                  owner: self)
        addTrackingArea(area)
        tracking = area
    }
    override func mouseEntered(with event: NSEvent) { hovered = true }
    override func mouseExited(with event: NSEvent) { hovered = false }

    @objc private func cancel() { onCancel?() }
}

/// “Replaced · ⌘Z to undo” (or “Copied · in History”) near the range end.
final class ReceiptView: ChipView {
    init(kind: OverlayReceipt, palette: OverlayPalette) {
        super.init(palette: palette, leading: 7, trailing: 8)
        let check = NSImageView()
        check.image = NSImage(systemSymbolName: kind == .replaced ? "checkmark" : "doc.on.doc", accessibilityDescription: nil)
        check.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 9.5, weight: .bold)
        check.contentTintColor = kind == .replaced ? (palette.dark ? NSColor(srgbRed: 0.14, green: 0.6, blue: 0.27, alpha: 1)
                                                                   : NSColor(srgbRed: 0.19, green: 0.82, blue: 0.35, alpha: 1))
                                                   : palette.chipText
        stack.spacing = 5
        stack.addArrangedSubview(check)
        switch kind {
        case .replaced:
            stack.addArrangedSubview(chipLabel("Replaced", weight: .semibold, color: palette.chipText))
            stack.addArrangedSubview(chipLabel("·", weight: .medium, color: palette.chipText.withAlphaComponent(0.6)))
            let keys = NSStackView(views: [KeycapView("⌘", style: .chip, palette: palette),
                                           KeycapView("Z", style: .chip, palette: palette)])
            keys.spacing = 2
            stack.addArrangedSubview(keys)
            stack.addArrangedSubview(chipLabel("to undo", weight: .medium, color: palette.chipText.withAlphaComponent(0.6)))
            setAccessibilityLabel("Replaced. Press Command Z to undo.")
        case .copied:
            stack.addArrangedSubview(chipLabel("Copied", weight: .semibold, color: palette.chipText))
            stack.addArrangedSubview(chipLabel("· kept in History", weight: .medium, color: palette.chipText.withAlphaComponent(0.6)))
            setAccessibilityLabel("Copied. The result is kept in History.")
        }
    }

    required init?(coder: NSCoder) { fatalError("unused") }
}

enum OverlayReceipt { case replaced, copied }

/// Owns every Ink Sweep window. Delayed work is scoped to a generation so an
/// old afterglow can never hide a newer command's overlay.
final class InkSweep {
    private(set) var highlightPanels: [OverlayPanel] = []
    let chipPanel = OverlayPanel(title: "Selara Chip", clickThrough: false)
    let receiptPanel = OverlayPanel(title: "Selara Receipt", clickThrough: true)
    private(set) var chip: SweepChipView?
    var appearance: NSAppearance?
    var onCancel: (() -> Void)?
    private var generation: UInt64 = 0

    var isShowingWork: Bool { chipPanel.isVisible }

    func showWorking(lines: [NSRect], command: String) {
        hide()
        let token = generation
        let palette = OverlayPalette.resolve(appearance)
        let reduce = Motion.reduceMotion
        highlightPanels = Self.panels(for: lines, title: "Selara Sweep", appearance: appearance) { layer, rect in
            let line = CALayer()
            line.frame = rect
            line.cornerRadius = 3
            line.masksToBounds = true
            line.backgroundColor = palette.tint
            if !reduce {
                let band = CAGradientLayer()
                let width = rect.width
                band.colors = [NSColor.clear.cgColor, palette.shimmer, NSColor.clear.cgColor]
                band.startPoint = CGPoint(x: 0, y: 0.45)
                band.endPoint = CGPoint(x: 1, y: 0.55)
                band.frame = CGRect(x: -width, y: 0, width: max(36, width * 0.75), height: rect.height)
                let sweep = CABasicAnimation(keyPath: "position.x")
                sweep.fromValue = -width
                sweep.toValue = width * 2
                sweep.duration = Motion.shimmerLoop
                sweep.repeatCount = .infinity
                sweep.timingFunction = CAMediaTimingFunction(name: .linear)
                band.add(sweep, forKey: "shimmer")
                line.addSublayer(band)
            }
            layer.addSublayer(line)
        }
        let chip = SweepChipView(command: command, palette: palette)
        chip.onCancel = { [weak self] in self?.onCancel?() }
        self.chip = chip
        let size = chip.fittingChipSize
        let visible = Self.visibleFrame(for: lines)
        chipPanel.appearance = appearance
        chipPanel.setFrame(NSRect(origin: SelectionGeometry.chipOrigin(size: size, lines: lines, visible: visible),
                                  size: size), display: false)
        chip.frame = NSRect(origin: .zero, size: size)
        chipPanel.contentView?.subviews.forEach { $0.removeFromSuperview() }
        chipPanel.contentView?.addSubview(chip)
        chip.orb.spin(!reduce)
        for panel in highlightPanels + [chipPanel] {
            panel.alphaValue = 0
            panel.orderFrontRegardless()
        }
        fade(highlightPanels + [chipPanel], to: 1, duration: Motion.duration(0.18), token: token)
        if !reduce, let layer = chip.layer {
            let rise = CABasicAnimation(keyPath: "transform.translation.y")
            rise.fromValue = -4
            rise.toValue = 0
            rise.duration = 0.18
            rise.timingFunction = Motion.easeOut
            layer.add(rise, forKey: "rise")
        }
    }

    /// Afterglow over `lines` (the replaced range) and the undo receipt.
    func succeed(lines: [NSRect]) {
        hide()
        let token = generation
        let palette = OverlayPalette.resolve(appearance)
        let reduce = Motion.reduceMotion
        let step = min(Motion.spring * 0.5, 0.6 / Double(max(1, lines.count)))
        var index = 0
        let start = CACurrentMediaTime()
        highlightPanels = Self.panels(for: lines, title: "Selara Afterglow", appearance: appearance) { layer, rect in
            let glow = CALayer()
            glow.frame = rect
            glow.cornerRadius = 3
            glow.backgroundColor = palette.glow
            if !reduce {
                let mask = CALayer()
                mask.backgroundColor = NSColor.black.cgColor
                mask.anchorPoint = CGPoint(x: 0, y: 0.5)
                mask.frame = CGRect(origin: .zero, size: rect.size)
                let wipe = CABasicAnimation(keyPath: "bounds.size.width")
                wipe.fromValue = 0
                wipe.toValue = rect.width
                wipe.beginTime = start + Double(index) * step
                wipe.duration = Motion.spring
                wipe.timingFunction = Motion.easeOut
                wipe.fillMode = .backwards
                mask.add(wipe, forKey: "wipe")
                glow.mask = mask
            }
            layer.addSublayer(glow)
            index += 1
        }
        let wipeEnd = reduce ? Motion.micro : Double(max(0, lines.count - 1)) * step + Motion.spring
        for panel in highlightPanels {
            panel.alphaValue = reduce ? 0 : 1
            panel.orderFrontRegardless()
        }
        if reduce { fade(highlightPanels, to: 1, duration: Motion.micro, token: token) }
        later(wipeEnd, token) { sweep in
            sweep.fade(sweep.highlightPanels, to: 0, duration: reduce ? Motion.micro : Motion.afterglowFade,
                       token: token) { sweep in sweep.highlightPanels.forEach { $0.orderOut(nil) } }
        }
        showReceipt(.replaced, lines: lines, token: token)
    }

    /// A receipt by itself (e.g. after ⌘C in the review card).
    func showReceipt(_ kind: OverlayReceipt, lines: [NSRect]) {
        hide()
        showReceipt(kind, lines: lines, token: generation)
    }

    private func showReceipt(_ kind: OverlayReceipt, lines: [NSRect], token: UInt64) {
        guard !lines.isEmpty else { return }
        let receipt = ReceiptView(kind: kind, palette: OverlayPalette.resolve(appearance))
        let size = receipt.fittingChipSize
        receipt.frame = NSRect(origin: .zero, size: size)
        receiptPanel.appearance = appearance
        receiptPanel.contentView?.subviews.forEach { $0.removeFromSuperview() }
        receiptPanel.contentView?.addSubview(receipt)
        let origin = SelectionGeometry.hintOrigin(size: size, lines: lines, visible: Self.visibleFrame(for: lines))
        receiptPanel.setFrame(NSRect(origin: origin, size: size), display: false)
        receiptPanel.alphaValue = 0
        receiptPanel.orderFrontRegardless()
        fade([receiptPanel], to: 1, duration: Motion.duration(0.2), token: token)
        later(Motion.hold, token) { sweep in
            sweep.fade([sweep.receiptPanel], to: 0, duration: Motion.duration(0.2), token: token) {
                $0.receiptPanel.orderOut(nil)
            }
        }
    }

    func hide() {
        generation &+= 1
        chip?.orb.spin(false)
        chip?.hovered = false
        for panel in highlightPanels { panel.orderOut(nil) }
        highlightPanels = []
        chipPanel.orderOut(nil)
        receiptPanel.orderOut(nil)
    }

    // MARK: Helpers

    /// One click-through panel per display, covering that display's lines.
    static func panels(for lines: [NSRect], title: String, appearance: NSAppearance?,
                       build: (CALayer, CGRect) -> Void) -> [OverlayPanel] {
        let screens = NSScreen.screens.map(\.frame)
        var groups: [Int: [NSRect]] = [:]
        for line in lines {
            let center = NSPoint(x: line.midX, y: line.midY)
            let index = screens.firstIndex(where: { $0.contains(center) })
                ?? screens.firstIndex(where: { $0.intersects(line) }) ?? 0
            groups[index, default: []].append(line)
        }
        return groups.keys.sorted().compactMap { key in
            guard let group = groups[key], let union = SelectionGeometry.union(group) else { return nil }
            let frame = union.insetBy(dx: -2, dy: -2).integral
            let panel = OverlayPanel(title: title, clickThrough: true)
            panel.appearance = appearance
            panel.setFrame(frame, display: false)
            guard let layer = panel.contentView?.layer else { return nil }
            for line in group {
                build(layer, CGRect(x: line.minX - frame.minX, y: line.minY - frame.minY,
                                    width: line.width, height: line.height))
            }
            return panel
        }
    }

    static func visibleFrame(for lines: [NSRect]) -> NSRect {
        let center = lines.first.map { NSPoint(x: $0.midX, y: $0.midY) } ?? NSEvent.mouseLocation
        let screen = NSScreen.screens.first(where: { $0.frame.contains(center) }) ?? NSScreen.main
        return screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
    }

    private func later(_ delay: TimeInterval, _ token: UInt64, _ action: @escaping (InkSweep) -> Void) {
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let self, self.generation == token else { return }
            action(self)
        }
    }

    private func fade(_ panels: [NSWindow], to target: CGFloat, duration: TimeInterval, token: UInt64,
                      completion: @escaping (InkSweep) -> Void = { _ in }) {
        guard duration > 0 else {
            panels.forEach { $0.alphaValue = target }
            completion(self)
            return
        }
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = duration
            context.timingFunction = Motion.easeOut
            panels.forEach { $0.animator().alphaValue = target }
        }, completionHandler: { [weak self] in
            guard let self, self.generation == token else { return }
            completion(self)
        })
    }
}
