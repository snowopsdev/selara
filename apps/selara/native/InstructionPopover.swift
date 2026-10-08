// Native custom-instruction popover (design direction 2B). Called only from
// the Rust service's AppKit thread, like Progress.swift. The panel never
// activates Selara: it becomes key so it can take typing while the source app
// stays frontmost, which keeps `return_to_source` and the paste path intact.
import AppKit
import ApplicationServices

// MARK: - Colors

private extension NSColor {
    static func instructionDynamic(light: NSColor, dark: NSColor) -> NSColor {
        NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
        }
    }
    /// Reference `--mk-group`: chip and field fill.
    static let instructionChip = instructionDynamic(light: .white, dark: NSColor(srgbRed: 0x2c / 255, green: 0x2c / 255, blue: 0x2f / 255, alpha: 1))
    /// Reference `--mk-field`.
    static let instructionField = instructionDynamic(light: .white, dark: NSColor(srgbRed: 0x1c / 255, green: 0x1c / 255, blue: 0x1e / 255, alpha: 1))
    /// Reference `--mk-line-strong`.
    static let instructionLine = instructionDynamic(light: NSColor.black.withAlphaComponent(0.16), dark: NSColor.white.withAlphaComponent(0.18))
    /// Lifts the popover material toward the reference's rgba(250,250,252,.9)
    /// and rgba(44,44,48,.9) while keeping the blur behind it.
    static let instructionTint = instructionDynamic(light: NSColor(srgbRed: 250 / 255, green: 250 / 255, blue: 252 / 255, alpha: 0.78),
                                                    dark: NSColor(srgbRed: 44 / 255, green: 44 / 255, blue: 48 / 255, alpha: 0.45))
    /// Reference popover hairline.
    static let instructionRim = instructionDynamic(light: NSColor.black.withAlphaComponent(0.16), dark: NSColor.white.withAlphaComponent(0.16))
}

private func instructionReduceMotion() -> Bool {
    NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
}

// MARK: - Panel

/// Borderless panels refuse key status by default; this one needs it for typing.
final class InstructionKeyPanel: NSPanel {
    var keyEquivalentHandler: ((NSEvent) -> Bool)?
    var escapeHandler: (() -> Void)?
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if keyEquivalentHandler?(event) == true { return true }
        return super.performKeyEquivalent(with: event)
    }
    override func cancelOperation(_ sender: Any?) { escapeHandler?() }
}

/// Popover material clipped to the bubble outline, with a hairline rim.
final class InstructionBubbleView: NSView {
    private let effect = NSVisualEffectView()
    private let tint = CAShapeLayer()
    private let rim = CAShapeLayer()
    private(set) var edge: InstructionArrowEdge = .top
    private(set) var arrowX: CGFloat = InstructionArrow.preferredOffset

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        effect.material = .popover
        effect.blendingMode = .behindWindow
        effect.state = .active
        effect.autoresizingMask = [.width, .height]
        effect.frame = bounds
        addSubview(effect)
        layer?.addSublayer(tint)
        rim.fillColor = nil
        rim.lineWidth = 0.5
        layer?.addSublayer(rim)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override var wantsUpdateLayer: Bool { true }

    func configure(edge: InstructionArrowEdge, arrowX: CGFloat) {
        self.edge = edge
        self.arrowX = arrowX
        needsLayout = true
        needsDisplay = true
    }

    override func layout() {
        super.layout()
        effect.frame = bounds
        let size = bounds.size, edge = edge, arrowX = arrowX
        effect.maskImage = NSImage(size: size, flipped: false) { _ in
            NSColor.black.setFill()
            let path = NSBezierPath()
            path.append(instructionBubblePath(size: size, edge: edge, arrowX: arrowX))
            path.fill()
            return true
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        let outline = instructionBubblePath(size: size, edge: edge, arrowX: arrowX)
        tint.frame = bounds
        tint.path = outline
        rim.frame = bounds
        rim.path = outline
        CATransaction.commit()
        window?.invalidateShadow()
    }

    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            tint.fillColor = NSColor.instructionTint.cgColor
            rim.strokeColor = NSColor.instructionRim.cgColor
        }
    }
}

// NSBezierPath(cgPath:) is macOS 14+; this keeps the 11.0 deployment target.
private extension NSBezierPath {
    func append(_ cgPath: CGPath) {
        cgPath.applyWithBlock { element in
            let points = element.pointee.points
            switch element.pointee.type {
            case .moveToPoint: move(to: points[0])
            case .addLineToPoint: line(to: points[0])
            case .addQuadCurveToPoint:
                let start = currentPoint
                let control = points[0], end = points[1]
                curve(to: end,
                      controlPoint1: NSPoint(x: start.x + 2 / 3 * (control.x - start.x), y: start.y + 2 / 3 * (control.y - start.y)),
                      controlPoint2: NSPoint(x: end.x + 2 / 3 * (control.x - end.x), y: end.y + 2 / 3 * (control.y - end.y)))
            case .addCurveToPoint: curve(to: points[2], controlPoint1: points[0], controlPoint2: points[1])
            case .closeSubpath: close()
            @unknown default: break
            }
        }
    }
}

// MARK: - Controls

/// Capsule intent chip: "Shorter 1". Selected = accent fill, white text.
final class InstructionChipButton: NSButton {
    let chip: InstructionChip
    var isOn = false { didSet { if isOn != oldValue { refresh(animated: true) } } }
    static let height: CGFloat = 26
    private static let font = NSFont.systemFont(ofSize: 12.5)
    private static let keyFont = NSFont.systemFont(ofSize: 10.5)

    init(chip: InstructionChip) {
        self.chip = chip
        super.init(frame: .zero)
        isBordered = false
        wantsLayer = true
        refusesFirstResponder = true
        setButtonType(.momentaryChange)
        (cell as? NSButtonCell)?.highlightsBy = []
        (cell as? NSButtonCell)?.showsStateBy = []
        focusRingType = .none
        layer?.cornerRadius = Self.height / 2
        layer?.borderWidth = 0.5
        toolTip = "\(chip.title) (\(chip.rawValue) or ⌘\(chip.rawValue))"
        setAccessibilityRole(.checkBox)
        setAccessibilityLabel(chip.title)
        refresh(animated: false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override var wantsUpdateLayer: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override var intrinsicContentSize: NSSize {
        NSSize(width: ceil(attributedTitle.size().width) + 20, height: Self.height)
    }

    private func refresh(animated: Bool) {
        let text: NSColor = isOn ? .white : .labelColor
        let title = NSMutableAttributedString(string: chip.title, attributes: [.font: Self.font, .foregroundColor: text])
        title.append(NSAttributedString(string: "  \(chip.rawValue)", attributes: [
            .font: Self.keyFont,
            .foregroundColor: isOn ? NSColor.white.withAlphaComponent(0.6) : NSColor.secondaryLabelColor,
            .baselineOffset: 0.5,
        ]))
        attributedTitle = title
        setAccessibilityValue(isOn)
        if animated, !instructionReduceMotion() {
            // AppKit-backed layers don't animate implicitly; crossfade the swap.
            let fade = CATransition()
            fade.type = .fade
            fade.duration = InstructionMotion.micro
            let ease = InstructionMotion.easeOut
            fade.timingFunction = CAMediaTimingFunction(controlPoints: ease.0, ease.1, ease.2, ease.3)
            layer?.add(fade, forKey: "chip")
        }
        needsDisplay = true
    }

    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = (isOn ? NSColor.controlAccentColor : NSColor.instructionChip).cgColor
            layer?.borderColor = (isOn ? NSColor.clear : NSColor.instructionLine).cgColor
        }
    }
}

/// Accent "Replace ↩" button inside the field (reference `.mk-btn.primary`).
final class InstructionPrimaryButton: NSButton {
    private var pressed = false { didSet { needsDisplay = true } }
    override init(frame: NSRect) {
        super.init(frame: frame)
        isBordered = false
        wantsLayer = true
        refusesFirstResponder = true
        focusRingType = .none
        (cell as? NSButtonCell)?.highlightsBy = []
        layer?.cornerRadius = 6
        attributedTitle = NSAttributedString(string: "Replace ↩", attributes: [
            .font: NSFont.systemFont(ofSize: 13), .foregroundColor: NSColor.white,
        ])
        setAccessibilityLabel("Replace selection")
        toolTip = "Replace the selection (↩)"
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override var wantsUpdateLayer: Bool { true }
    override var isEnabled: Bool { didSet { needsDisplay = true } }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var intrinsicContentSize: NSSize {
        NSSize(width: ceil(attributedTitle.size().width) + 24, height: 22)
    }
    override func mouseDown(with event: NSEvent) {
        pressed = true
        super.mouseDown(with: event)
        pressed = false
    }
    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            let accent = NSColor.controlAccentColor
            layer?.backgroundColor = (pressed ? accent.blended(withFraction: 0.18, of: .black) ?? accent : accent).cgColor
        }
        alphaValue = isEnabled ? 1 : 0.45
    }
}

/// Rounded field chrome (reference `.ib-field`).
final class InstructionFieldView: NSView {
    var onClick: (() -> Void)?
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 9
        layer?.borderWidth = 0.5
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    override var wantsUpdateLayer: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) { onClick?() }
    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = NSColor.instructionField.cgColor
            layer?.borderColor = NSColor.instructionLine.cgColor
        }
    }
}

/// Lets bare number keys toggle chips before any detail has been typed.
final class InstructionTextView: NSTextView {
    var numberKeyHandler: ((NSEvent) -> Bool)?
    override func keyDown(with event: NSEvent) {
        if numberKeyHandler?(event) == true { return }
        super.keyDown(with: event)
    }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

// MARK: - Controller

final class InstructionPopoverController: NSObject, NSTextViewDelegate, NSWindowDelegate {
    static let width: CGFloat = 470
    private static let padding: CGFloat = 12
    private static let fieldFont = NSFont.systemFont(ofSize: 13.5)
    private static let metaFont = NSFont.systemFont(ofSize: 11.5)

    let panel: InstructionKeyPanel
    private let bubble = InstructionBubbleView()
    private let stage = NSView()
    private var chipButtons: [InstructionChipButton] = []
    private let field = InstructionFieldView()
    private let scroll = NSScrollView()
    private let textView: InstructionTextView
    private let placeholder = NSTextField(labelWithString: "Or describe the change…")
    private let replaceButton = InstructionPrimaryButton()
    private let metaLeft = NSTextField(labelWithString: "")
    private let metaRight = NSTextField(labelWithString: "")
    private let undo = UndoManager()

    // Popover state.
    private(set) var chips = InstructionChips()
    private var prefixLength = 0
    private var history: [String] = []
    private(set) var historyCursor: Int?
    private var editingProgrammatically = false
    private var notice: (text: String, success: Bool)?
    private var noticeGeneration: UInt64 = 0
    private var shownAt: TimeInterval = 0
    private var closing = true
    private var anchor: NSRect?

    // Rust plumbing: an event queue drained every frame, plus a wake hook so
    // egui repaints (and polls) right away instead of on its idle timer.
    private var events: [InstructionEvent] = []
    var wake: (() -> Void)?

    override init() {
        precondition(Thread.isMainThread)
        panel = InstructionKeyPanel(contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 120),
                                    styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        let storage = NSTextStorage()
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let container = NSTextContainer(size: NSSize(width: 300, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = true
        container.lineFragmentPadding = 0
        layout.addTextContainer(container)
        textView = InstructionTextView(frame: .zero, textContainer: container)
        super.init()
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isReleasedWhenClosed = false
        panel.hidesOnDeactivate = false
        panel.becomesKeyOnlyIfNeeded = false
        panel.level = .floating
        panel.collectionBehavior = [.fullScreenAuxiliary, .moveToActiveSpace]
        panel.animationBehavior = .none
        panel.title = "Custom instruction"
        panel.delegate = self
        panel.keyEquivalentHandler = { [weak self] in self?.handleKeyEquivalent($0) ?? false }
        panel.escapeHandler = { [weak self] in self?.cancel() }

        let root = NSView(frame: panel.contentRect(forFrameRect: panel.frame))
        root.wantsLayer = true
        panel.contentView = root
        stage.wantsLayer = true
        stage.frame = root.bounds
        stage.autoresizingMask = [.width, .height]
        root.addSubview(stage)
        bubble.frame = stage.bounds
        bubble.autoresizingMask = [.width, .height]
        stage.addSubview(bubble)

        for chip in InstructionChip.allCases {
            let button = InstructionChipButton(chip: chip)
            button.target = self
            button.action = #selector(chipClicked(_:))
            chipButtons.append(button)
            stage.addSubview(button)
        }

        field.onClick = { [weak self] in self?.focusText() }
        stage.addSubview(field)
        textView.font = Self.fieldFont
        textView.textColor = .labelColor
        textView.insertionPointColor = .controlAccentColor
        textView.drawsBackground = false
        textView.isRichText = false
        textView.allowsUndo = true
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.textContainerInset = .zero
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.focusRingType = .none
        textView.typingAttributes = [.font: Self.fieldFont, .foregroundColor: NSColor.labelColor]
        textView.delegate = self
        textView.setAccessibilityLabel("Instruction")
        textView.setAccessibilityHelp("Return replaces the selection. Shift-Return adds a line.")
        textView.numberKeyHandler = { [weak self] in self?.handleNumberKey($0) ?? false }
        scroll.documentView = textView
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasVerticalScroller = false
        scroll.verticalScrollElasticity = .none
        field.addSubview(scroll)
        placeholder.font = Self.fieldFont
        placeholder.textColor = .placeholderTextColor
        placeholder.isSelectable = false
        placeholder.setAccessibilityElement(false)
        field.addSubview(placeholder)
        replaceButton.target = self
        replaceButton.action = #selector(submitClicked)
        field.addSubview(replaceButton)

        for label in [metaLeft, metaRight] {
            label.font = Self.metaFont
            label.textColor = .secondaryLabelColor
            label.lineBreakMode = .byTruncatingTail
            stage.addSubview(label)
        }
        metaRight.alignment = .right
    }

    // MARK: Showing

    /// Capture before any Selara UI exists. Mirrors Progress.swift's AX
    /// selection-bounds lookup; the pointer is the fallback anchor.
    func captureAnchor(sourcePID: Int32) {
        anchor = NSRect(origin: NSEvent.mouseLocation, size: .zero)
        guard sourcePID > 0,
              NSWorkspace.shared.frontmostApplication?.processIdentifier == sourcePID else { return }
        let app = AXUIElementCreateApplication(sourcePID)
        AXUIElementSetMessagingTimeout(app, 0.06)
        var focused: CFTypeRef?
        guard AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute as CFString, &focused) == .success,
              let focused, CFGetTypeID(focused) == AXUIElementGetTypeID() else { return }
        let element = unsafeBitCast(focused, to: AXUIElement.self)
        AXUIElementSetMessagingTimeout(element, 0.06)
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
              let selection = Self.screenRect(rect) else { return }
        anchor = selection
    }

    /// AX top-left global coordinates → AppKit screen points. Rejects bounds
    /// that are degenerate or off every screen (scrolled-out document rects).
    private static func screenRect(_ rect: CGRect) -> NSRect? {
        guard rect.width >= 0, rect.height > 0,
              [rect.minX, rect.minY, rect.width, rect.height].allSatisfy({ $0.isFinite }),
              let primary = NSScreen.screens.first else { return nil }
        let converted = NSRect(x: rect.minX, y: primary.frame.maxY - rect.maxY,
                               width: rect.width, height: rect.height)
        guard NSScreen.screens.contains(where: { screen in
            screen.visibleFrame.intersects(converted) || screen.visibleFrame.contains(converted.origin)
        }) else { return nil }
        return converted
    }

    var isVisible: Bool { panel.isVisible && !closing }

    /// Open the popover for a fresh selection. `history` is newest first.
    func present(app: String?, chars: UInt64, history: [String], anchor explicitAnchor: NSRect? = nil) {
        hide()
        if let explicitAnchor { anchor = explicitAnchor }
        self.history = history
        historyCursor = nil
        chips.removeAll()
        notice = nil
        noticeGeneration &+= 1
        setText("", prefixLength: 0)
        metaRight.stringValue = instructionSelectionSummary(app: app, chars: chars)
        refreshChrome()
        let body = layoutBody()
        let target = anchor ?? NSRect(origin: NSEvent.mouseLocation, size: .zero)
        let visible = Self.screen(for: target)?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let placement = instructionPopoverPlacement(anchor: target, body: body, visible: visible)
        apply(placement)
        closing = false
        shownAt = ProcessInfo.processInfo.systemUptime
        panel.alphaValue = 0
        panel.makeKeyAndOrderFront(nil)
        panel.makeFirstResponder(textView)
        animateIn()
    }

    func hide() {
        closing = true
        stage.layer?.removeAllAnimations()
        panel.orderOut(nil)
        panel.alphaValue = 1
    }

    private static func screen(for anchor: NSRect) -> NSScreen? {
        let center = NSPoint(x: anchor.midX, y: anchor.midY)
        return NSScreen.screens.first(where: { $0.frame.contains(center) })
            ?? NSScreen.screens.max(by: { left, right in
                func area(_ screen: NSScreen) -> CGFloat {
                    let overlap = screen.visibleFrame.intersection(anchor)
                    return overlap.isNull ? 0 : overlap.width * overlap.height
                }
                return area(left) < area(right)
            })
            ?? NSScreen.main
    }

    private func apply(_ placement: InstructionPlacement) {
        bubble.configure(edge: placement.edge, arrowX: placement.arrowX)
        panel.setFrame(placement.frame, display: false)
        layoutContent()
    }

    /// 240 ms scale + opacity from the arrow tip; Reduce Motion: 120 ms fade.
    private func animateIn() {
        let reduce = instructionReduceMotion()
        NSAnimationContext.runAnimationGroup { context in
            context.duration = reduce ? InstructionMotion.reducedFade : InstructionMotion.panel
            let ease = InstructionMotion.easeOut
            context.timingFunction = CAMediaTimingFunction(controlPoints: ease.0, ease.1, ease.2, ease.3)
            panel.animator().alphaValue = 1
        }
        guard !reduce, let layer = stage.layer else { return }
        let size = stage.bounds.size
        let tip = CGPoint(x: bubble.arrowX, y: bubble.edge == .top ? size.height : 0)
        let anchorPoint = CGPoint(x: layer.anchorPoint.x * size.width, y: layer.anchorPoint.y * size.height)
        let animation = CAKeyframeAnimation(keyPath: "transform")
        animation.values = InstructionMotion.appearScale.map { scale in
            // Scale about the arrow tip, whatever the layer's anchor point is.
            let shift = CATransform3DMakeTranslation((1 - scale) * (tip.x - anchorPoint.x),
                                                     (1 - scale) * (tip.y - anchorPoint.y), 0)
            return NSValue(caTransform3D: CATransform3DConcat(CATransform3DMakeScale(scale, scale, 1), shift))
        }
        animation.keyTimes = InstructionMotion.appearKeyTimes.map { NSNumber(value: $0) }
        animation.timingFunctions = [CAMediaTimingFunction(name: .easeOut), CAMediaTimingFunction(name: .easeInEaseOut)]
        animation.duration = InstructionMotion.panel
        layer.add(animation, forKey: "appear")
    }

    // MARK: Layout

    private var lineHeight: CGFloat { ceil(NSLayoutManager().defaultLineHeight(for: Self.fieldFont)) }

    /// Height of the text, one to three lines; beyond that the field scrolls.
    private func textHeight() -> CGFloat {
        guard let layout = textView.layoutManager, let container = textView.textContainer else { return lineHeight }
        layout.ensureLayout(for: container)
        let used = ceil(layout.usedRect(for: container).height)
        return min(max(used, lineHeight), lineHeight * 3)
    }

    private func chipRows(width: CGFloat) -> [[InstructionChipButton]] {
        var rows: [[InstructionChipButton]] = [[]]
        var x: CGFloat = 0
        for button in chipButtons {
            let w = button.intrinsicContentSize.width
            if x > 0, x + w > width {
                rows.append([])
                x = 0
            }
            rows[rows.count - 1].append(button)
            x += w + 6
        }
        return rows
    }

    private var fieldHeight: CGFloat { max(38, textHeight() + 20) }

    /// Body size (without the arrow) for the current content.
    private func layoutBody() -> CGSize {
        let inner = Self.width - Self.padding * 2
        let rows = CGFloat(chipRows(width: inner).count)
        let chipsHeight = rows * InstructionChipButton.height + (rows - 1) * 6
        let height = Self.padding + chipsHeight + 10 + fieldHeight + 8 + 15 + Self.padding
        return CGSize(width: Self.width, height: ceil(height))
    }

    private func layoutContent() {
        let pad = Self.padding
        let inner = Self.width - pad * 2
        let bodyMinY: CGFloat = bubble.edge == .bottom ? InstructionArrow.height : 0
        let bodyMaxY = stage.bounds.height - (bubble.edge == .top ? InstructionArrow.height : 0)

        var y = bodyMaxY - pad
        for row in chipRows(width: inner) {
            y -= InstructionChipButton.height
            var x = pad
            for button in row {
                let w = button.intrinsicContentSize.width
                button.frame = NSRect(x: x, y: y, width: w, height: InstructionChipButton.height)
                x += w + 6
            }
            y -= 6
        }
        y += 6 - 10

        let fieldH = fieldHeight
        y -= fieldH
        field.frame = NSRect(x: pad, y: y, width: inner, height: fieldH)
        let button = replaceButton.intrinsicContentSize
        replaceButton.frame = NSRect(x: inner - 8 - button.width, y: (fieldH - button.height) / 2,
                                     width: button.width, height: button.height)
        let textWidth = replaceButton.frame.minX - 8 - 10
        let textH = textHeight()
        scroll.frame = NSRect(x: 10, y: (fieldH - textH) / 2, width: textWidth, height: textH)
        textView.frame.size.width = textWidth
        textView.minSize = NSSize(width: textWidth, height: textH)
        textView.maxSize = NSSize(width: textWidth, height: .greatestFiniteMagnitude)
        textView.sizeToFit()
        textView.scrollRangeToVisible(textView.selectedRange())
        placeholder.sizeToFit()
        placeholder.frame.origin = NSPoint(x: 10, y: (fieldH - placeholder.frame.height) / 2)

        let metaY = bodyMinY + pad
        let rightWidth = min(ceil(metaRight.fittingSize.width) + 6, inner / 2)
        metaRight.frame = NSRect(x: pad + inner - rightWidth, y: metaY, width: rightWidth, height: 15)
        metaLeft.frame = NSRect(x: pad, y: metaY, width: inner - rightWidth - 12, height: 15)
    }

    /// Re-measure after text or chip changes, keeping the arrow side pinned.
    private func relayout() {
        guard panel.isVisible else { return }
        let body = layoutBody()
        let height = body.height + InstructionArrow.height
        var frame = panel.frame
        if abs(frame.height - height) > 0.5 {
            if bubble.edge == .top { frame.origin.y = frame.maxY - height }
            frame.size.height = height
            panel.setFrame(frame, display: true)
        }
        layoutContent()
    }

    // MARK: Text model

    private var fullText: String { textView.string }
    private var detail: String { (textView.string as NSString).substring(from: min(prefixLength, (textView.string as NSString).length)) }
    private var detailIsEmpty: Bool { detail.isEmpty }
    /// What ↩ sends: compiled chips + space + typed detail.
    var instruction: String { composeInstruction(compiled: compileInstructionChips(chips), detail: detail) }

    private func setText(_ text: String, prefixLength: Int) {
        editingProgrammatically = true
        textView.textStorage?.setAttributedString(NSAttributedString(string: text, attributes: textView.typingAttributes))
        self.prefixLength = prefixLength
        textView.setSelectedRange(NSRange(location: (text as NSString).length, length: 0))
        undo.removeAllActions()
        editingProgrammatically = false
    }

    private func applyChips() {
        let old = textView.selectedRange()
        let caret = max(0, old.location - prefixLength)
        let prefix = instructionPrefix(chips)
        let length = (prefix as NSString).length
        editingProgrammatically = true
        textView.textStorage?.replaceCharacters(in: NSRange(location: 0, length: prefixLength),
                                                with: NSAttributedString(string: prefix, attributes: textView.typingAttributes))
        prefixLength = length
        textView.setSelectedRange(NSRange(location: length + caret, length: 0))
        undo.removeAllActions()
        editingProgrammatically = false
        historyCursor = nil
        refreshChrome()
        relayout()
    }

    func toggle(_ chip: InstructionChip) {
        chips.toggle(chip)
        applyChips()
    }

    /// ↑ (`delta > 0`) older, ↓ newer. A recalled instruction replaces chips.
    func recall(_ delta: Int) {
        let next = instructionHistoryStep(historyCursor, count: history.count, delta: delta)
        chips.removeAll()
        setText(next.map { history[$0] } ?? "", prefixLength: 0)
        historyCursor = next
        refreshChrome()
        relayout()
    }

    func setDetail(_ text: String) {
        textView.setSelectedRange(NSRange(location: prefixLength, length: (fullText as NSString).length - prefixLength))
        textView.insertText(text, replacementRange: textView.selectedRange())
    }

    private func refreshChrome() {
        for button in chipButtons { button.isOn = chips.contains(button.chip) }
        placeholder.isHidden = !fullText.isEmpty
        replaceButton.isEnabled = !instruction.isEmpty
        if let notice {
            let text = NSMutableAttributedString()
            if notice.success {
                text.append(NSAttributedString(string: "✓ ", attributes: [.foregroundColor: NSColor.systemGreen, .font: Self.metaFont]))
            }
            text.append(NSAttributedString(string: notice.text, attributes: [
                .foregroundColor: notice.success ? NSColor.secondaryLabelColor : NSColor.systemRed, .font: Self.metaFont,
            ]))
            metaLeft.attributedStringValue = text
        } else if let cursor = historyCursor {
            metaLeft.stringValue = "Recent \(cursor + 1) of \(history.count) · ↑ older · ↓ newer"
        } else if history.isEmpty {
            metaLeft.stringValue = "Number keys toggle chips · ⌘S saves as command"
        } else {
            metaLeft.stringValue = "Number keys toggle chips · ↑ recalls recent"
        }
    }

    /// Transient meta-row receipt for ⌘S; errors stay until the next change.
    func showNotice(_ text: String, success: Bool) {
        notice = (text, success)
        noticeGeneration &+= 1
        let token = noticeGeneration
        refreshChrome()
        guard success else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + InstructionMotion.hold) { [weak self] in
            guard let self, self.noticeGeneration == token else { return }
            self.notice = nil
            if instructionReduceMotion() { self.refreshChrome(); return }
            NSAnimationContext.runAnimationGroup({ context in
                context.duration = InstructionMotion.micro / 2
                self.metaLeft.animator().alphaValue = 0
            }, completionHandler: {
                self.refreshChrome()
                NSAnimationContext.runAnimationGroup { context in
                    context.duration = InstructionMotion.micro / 2
                    self.metaLeft.animator().alphaValue = 1
                }
            })
        }
    }

    // MARK: Actions

    private func focusText() { panel.makeFirstResponder(textView) }

    @objc private func chipClicked(_ sender: InstructionChipButton) {
        toggle(sender.chip)
        focusText()
    }

    @objc private func submitClicked() { submit() }

    private func emit(_ event: InstructionEvent) {
        events.append(event)
        wake?()
    }

    func takeEvent() -> InstructionEvent? { events.isEmpty ? nil : events.removeFirst() }

    func submit() {
        let text = instruction
        guard !text.isEmpty, !closing else { return }
        // Give keyboard focus back to the source app before Rust revalidates
        // the selection and, later, pastes.
        hide()
        emit(.submit(text))
    }

    func save() {
        let text = instruction
        guard !text.isEmpty, !closing else { NSSound.beep(); return }
        emit(.save(text))
    }

    func cancel() {
        guard !closing else { return }
        hide()
        emit(.cancel)
    }

    // MARK: Keys

    private func handleNumberKey(_ event: NSEvent) -> Bool {
        let mods = event.modifierFlags.intersection([.command, .control, .option, .shift])
        guard mods.isEmpty, detailIsEmpty, historyCursor == nil,
              let key = event.charactersIgnoringModifiers, let chip = InstructionChip(key: key) else { return false }
        toggle(chip)
        return true
    }

    private func handleKeyEquivalent(_ event: NSEvent) -> Bool {
        guard event.type == .keyDown, !closing else { return false }
        let mods = event.modifierFlags.intersection([.command, .control, .option, .shift])
        let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
        if mods == .command, let chip = InstructionChip(key: key) { toggle(chip); return true }
        switch (mods, key) {
        case (.command, "s"): save(); return true
        case (.command, "\r"): submit(); return true
        case (.command, "w"), (.command, "."): cancel(); return true
        case (.command, "a"): textView.selectAll(nil); return true
        case (.command, "c"): textView.copy(nil); return true
        case (.command, "x"): textView.cut(nil); return true
        case (.command, "v"): textView.pasteAsPlainText(nil); return true
        case (.command, "z"): undo.undo(); return true
        case ([.command, .shift], "z"): undo.redo(); return true
        default: return false
        }
    }

    func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            let mods = NSApp.currentEvent?.modifierFlags ?? []
            if mods.contains(.shift) || mods.contains(.option) {
                textView.insertNewlineIgnoringFieldEditor(nil)
            } else {
                submit()
            }
            return true
        case #selector(NSResponder.cancelOperation(_:)), #selector(NSTextView.complete(_:)):
            cancel()
            return true
        case #selector(NSResponder.moveUp(_:)):
            guard fullText.isEmpty || historyCursor != nil, !history.isEmpty else { return false }
            recall(1)
            return true
        case #selector(NSResponder.moveDown(_:)):
            guard historyCursor != nil else { return false }
            recall(-1)
            return true
        case #selector(NSResponder.deleteBackward(_:)):
            let selection = textView.selectedRange()
            guard selection.length == 0, selection.location == prefixLength, !chips.isEmpty else { return false }
            chips.removeLast()
            applyChips()
            return true
        default:
            return false
        }
    }

    // The compiled sentence is owned by the chips: edits start after it.
    func textView(_ textView: NSTextView, shouldChangeTextIn range: NSRange, replacementString: String?) -> Bool {
        editingProgrammatically || range.location >= prefixLength
    }

    func textView(_ textView: NSTextView, willChangeSelectionFromCharacterRange old: NSRange,
                  toCharacterRange new: NSRange) -> NSRange {
        guard !editingProgrammatically, new.location < prefixLength else { return new }
        let end = max(NSMaxRange(new), prefixLength)
        return NSRange(location: prefixLength, length: end - prefixLength)
    }

    func textDidChange(_ notification: Notification) {
        guard !editingProgrammatically else { return }
        historyCursor = nil
        if notice != nil, notice?.success == false { notice = nil }
        refreshChrome()
        relayout()
    }

    func undoManager(for view: NSTextView) -> UndoManager? { undo }

    // MARK: Window

    /// A click elsewhere closes the popover like a transient NSPopover. A
    /// resign right after showing is AppKit settling key status; reclaim it.
    func windowDidResignKey(_ notification: Notification) {
        guard !closing, panel.isVisible else { return }
        if ProcessInfo.processInfo.systemUptime - shownAt < 0.25 {
            DispatchQueue.main.async { [weak self] in
                guard let self, !self.closing else { return }
                self.panel.makeKeyAndOrderFront(nil)
                self.panel.makeFirstResponder(self.textView)
            }
            return
        }
        hide()
        emit(.dismiss)
    }
}

// MARK: - C ABI (Rust: apps/selara/src/instruction.rs)

// One retained handle crosses the C ABI. All access, including release, stays
// on the main thread; the Rust owner is !Send and !Sync.
private func instructionController(_ handle: UnsafeMutableRawPointer) -> InstructionPopoverController {
    precondition(Thread.isMainThread)
    return Unmanaged<InstructionPopoverController>.fromOpaque(handle).takeUnretainedValue()
}

@_cdecl("selara_instruction_create")
public func instructionCreate(_ wake: (@convention(c) (UnsafeMutableRawPointer?) -> Void)?,
                              _ context: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer {
    precondition(Thread.isMainThread)
    let controller = InstructionPopoverController()
    if let wake { controller.wake = { wake(context) } }
    return Unmanaged.passRetained(controller).toOpaque()
}

@_cdecl("selara_instruction_destroy")
public func instructionDestroy(_ handle: UnsafeMutableRawPointer) {
    let controller = instructionController(handle)
    controller.wake = nil
    controller.hide()
    Unmanaged<InstructionPopoverController>.fromOpaque(handle).release()
}

@_cdecl("selara_instruction_capture_anchor")
public func instructionCaptureAnchor(_ handle: UnsafeMutableRawPointer, _ sourcePID: Int32) {
    instructionController(handle).captureAnchor(sourcePID: sourcePID)
}

/// `app` may be NULL. `history` holds `count` UTF-8 strings, newest first;
/// everything is copied before this returns.
@_cdecl("selara_instruction_show")
public func instructionShow(_ handle: UnsafeMutableRawPointer, _ app: UnsafePointer<CChar>?, _ chars: UInt64,
                            _ history: UnsafePointer<UnsafePointer<CChar>?>?, _ count: Int) {
    var items: [String] = []
    if let history {
        for index in 0 ..< max(0, count) {
            if let item = history[index] { items.append(String(cString: item)) }
        }
    }
    instructionController(handle).present(app: app.map { String(cString: $0) }, chars: chars, history: items)
}

@_cdecl("selara_instruction_hide")
public func instructionHide(_ handle: UnsafeMutableRawPointer) { instructionController(handle).hide() }

@_cdecl("selara_instruction_is_visible")
public func instructionIsVisible(_ handle: UnsafeMutableRawPointer) -> Bool {
    instructionController(handle).isVisible
}

@_cdecl("selara_instruction_set_notice")
public func instructionSetNotice(_ handle: UnsafeMutableRawPointer, _ text: UnsafePointer<CChar>, _ success: Bool) {
    instructionController(handle).showNotice(String(cString: text), success: success)
}

/// Next queued event as a malloc'd UTF-8 string (`kind` or `kind\ntext`),
/// or NULL. The caller frees it with `free`.
@_cdecl("selara_instruction_take_event")
public func instructionTakeEvent(_ handle: UnsafeMutableRawPointer) -> UnsafeMutablePointer<CChar>? {
    guard let event = instructionController(handle).takeEvent() else { return nil }
    return strdup(event.encoded)
}
