// 1D Ghost Diff: for commands with `review = true`, an anchored card shows a
// skeleton while the model works, then a word-level diff. ↩ replaces, ⇥ asks
// for another take, ⌘C copies, Esc discards. The card is a non-activating
// panel that can become key, so it receives those keys without activating
// Selara or changing the frontmost app. Main thread only.
import AppKit
import QuartzCore

/// C ABI codes shared with `ReviewAction::from_code` in apps/selara/src/review.rs.
enum ReviewActionCode: Int32 {
    case none = 0, accept = 1, anotherTake = 2, copy = 3, discard = 4, dismissed = 5
}

/// The JSON payload built by `review::diff_payload`.
struct DiffPayload: Decodable, Equatable {
    struct Segment: Decodable, Equatable {
        enum Op: String, Decodable { case equal, delete, insert }
        let op: Op
        let text: String
    }
    let segments: [Segment]
    let delta: Int
    let result: String

    static func decode(_ json: String) -> DiffPayload? {
        guard let data = json.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(DiffPayload.self, from: data)
    }

    /// “−68 chars”, “+12 chars”, “No length change”.
    var deltaLabel: String {
        if delta == 0 { return "Same length" }
        let count = NumberFormatter.localizedString(from: NSNumber(value: abs(delta)), number: .decimal)
        return "\(delta < 0 ? "−" : "+")\(count) \(abs(delta) == 1 ? "char" : "chars")"
    }
}

/// Key panel for the card. Keys never reach the source app while it is key.
final class ReviewPanel: NSPanel {
    var onKey: ((ReviewActionCode) -> Void)?
    var onResignKey: (() -> Void)?
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    override func keyDown(with event: NSEvent) {
        let modifiers = event.modifierFlags.intersection([.command, .option, .control, .shift])
        switch (event.keyCode, modifiers.isEmpty) {
        case (36, true), (76, true): onKey?(.accept)
        case (48, true): onKey?(.anotherTake)
        case (53, true): onKey?(.discard)
        default: break // Swallow: typing must not leak anywhere.
        }
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let modifiers = event.modifierFlags.intersection([.command, .option, .control, .shift])
        if modifiers == .command, event.charactersIgnoringModifiers?.lowercased() == "c" {
            onKey?(.copy)
            return true
        }
        return super.performKeyEquivalent(with: event)
    }

    override func cancelOperation(_ sender: Any?) { onKey?(.discard) }

    override func resignKey() {
        super.resignKey()
        onResignKey?()
    }
}

/// Draws inserted-text backgrounds as rounded pills, like the reference `ins`.
final class RoundedBackgroundLayoutManager: NSLayoutManager {
    override func fillBackgroundRectArray(_ rectArray: UnsafePointer<NSRect>, count rectCount: Int,
                                          forCharacterRange charRange: NSRange, color: NSColor) {
        color.setFill()
        for index in 0..<rectCount {
            let rect = rectArray[index].insetBy(dx: -2, dy: 1)
            NSBezierPath(roundedRect: rect, xRadius: 3, yRadius: 3).fill()
        }
    }
}

private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

/// Fills the card shape over the material (flipped coordinates).
private final class CardWashView: NSView {
    var shape: CGPath? { didSet { needsDisplay = true } }
    var color = NSColor.clear { didSet { needsDisplay = true } }
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        guard let shape, let context = NSGraphicsContext.current?.cgContext else { return }
        context.addPath(shape)
        context.setFillColor(color.cgColor)
        context.fillPath()
    }
}

/// Footer item: keycap plus label, clickable for pointer users.
private final class FooterAction: NSView {
    let code: ReviewActionCode
    var onClick: ((ReviewActionCode) -> Void)?
    private let label: NSTextField
    private var tracking: NSTrackingArea?

    init(keys: String, title: String, code: ReviewActionCode, palette: OverlayPalette) {
        self.code = code
        label = NSTextField(labelWithString: title)
        super.init(frame: .zero)
        label.font = .systemFont(ofSize: 11.5)
        label.textColor = .secondaryLabelColor
        let stack = NSStackView(views: [KeycapView(keys, style: .card, palette: palette), label])
        stack.spacing = 5
        stack.alignment = .centerY
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor), stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor), stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
        setAccessibilityLabel(title)
        toolTip = "\(title) (\(keys))"
    }

    required init?(coder: NSCoder) { fatalError("unused") }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds, options: [.activeAlways, .inVisibleRect, .mouseEnteredAndExited],
                                  owner: self)
        addTrackingArea(area)
        tracking = area
    }
    override func mouseEntered(with event: NSEvent) { label.textColor = .labelColor }
    override func mouseExited(with event: NSEvent) { label.textColor = .secondaryLabelColor }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {}
    override func mouseUp(with event: NSEvent) {
        if bounds.contains(convert(event.locationInWindow, from: nil)) { onClick?(code) }
    }
    override func accessibilityPerformPress() -> Bool { onClick?(code); return true }
}

/// Card geometry from the reference (variant D): 470 wide, 12 radius,
/// 12/14/10 padding, 13.5 pt diff text, keycap footer.
enum ReviewCardMetrics {
    static let width: CGFloat = 470
    static let radius: CGFloat = 12
    static let arrowWidth: CGFloat = 16
    static let arrowHeight: CGFloat = 7
    static let paddingTop: CGFloat = 12
    static let paddingX: CGFloat = 14
    static let paddingBottom: CGFloat = 10
    static let headerHeight: CGFloat = 15
    static let headerGap: CGFloat = 8
    static let skeletonHeight: CGFloat = 41
    static let footerHeight: CGFloat = 10 + 8 + 20
    static let maxDiffHeight: CGFloat = 240
    static let diffFontSize: CGFloat = 13.5
    static var contentWidth: CGFloat { width - 2 * paddingX }
}

final class ReviewCard {
    enum State: Equatable { case hidden, skeleton, diff }

    let panel: ReviewPanel
    private(set) var state: State = .hidden
    private(set) var payload: DiffPayload?
    var appearance: NSAppearance?
    var onAction: ((ReviewActionCode) -> Void)?
    private var generation: UInt64 = 0
    private var title = ""
    private var model = ""
    private var anchor = NSRect.zero
    private var pointX: CGFloat = 0
    /// Programmatic key changes (rerun, close) are not user dismissals.
    private var ignoreResign = false

    private let effect = NSVisualEffectView()
    /// Lifts the popover material toward the reference's solid card color
    /// while keeping vibrancy behind it.
    private let wash = CardWashView()
    private let border = CAShapeLayer()
    private let body = FlippedView()

    init() {
        panel = ReviewPanel(contentRect: NSRect(x: 0, y: 0, width: ReviewCardMetrics.width, height: 120),
                            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.title = "Selara Review"
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isReleasedWhenClosed = false
        panel.hidesOnDeactivate = false
        panel.becomesKeyOnlyIfNeeded = false
        panel.level = .floating
        panel.collectionBehavior = [.fullScreenAuxiliary, .moveToActiveSpace, .ignoresCycle, .transient]
        panel.animationBehavior = .none
        panel.isMovableByWindowBackground = false
        let content = FlippedView(frame: NSRect(x: 0, y: 0, width: ReviewCardMetrics.width, height: 120))
        content.wantsLayer = true
        panel.contentView = content
        effect.material = .popover
        effect.blendingMode = .behindWindow
        effect.state = .active
        content.addSubview(effect)
        content.addSubview(wash)
        content.addSubview(body)
        content.layer?.addSublayer(border)
        border.fillColor = nil
        border.lineWidth = 1
        panel.onKey = { [weak self] code in self?.handle(code) }
        panel.onResignKey = { [weak self] in
            guard let self, !self.ignoreResign, self.state == .diff else { return }
            self.handle(.dismissed)
        }
        panel.setAccessibilityLabel("Review rewrite")
    }

    var isVisible: Bool { panel.isVisible }

    /// Show the skeleton anchored to `anchor` (AppKit rect of the selection).
    /// A visible card switches to the skeleton immediately (another take).
    func showSkeleton(title: String, model: String, anchor: NSRect, pointX: CGFloat, delay: TimeInterval) {
        let wasVisible = panel.isVisible
        if wasVisible { resignKeyQuietly() }
        generation &+= 1
        let token = generation
        self.title = title
        self.model = model
        self.anchor = anchor
        self.pointX = pointX
        payload = nil
        state = .skeleton
        let present = { [weak self] in
            guard let self, self.generation == token else { return }
            self.render()
            if !wasVisible { self.appear(token: token) }
        }
        if wasVisible || delay <= 0 { present() } else {
            DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: present)
        }
    }

    /// Replace the skeleton with the diff and take key focus for ↩ ⇥ ⌘C Esc.
    @discardableResult
    func showDiff(_ payload: DiffPayload) -> Bool {
        guard state != .hidden else { return false }
        generation &+= 1
        let token = generation
        self.payload = payload
        state = .diff
        render()
        if !panel.isVisible { appear(token: token) }
        panel.makeKeyAndOrderFront(nil)
        panel.makeFirstResponder(nil)
        return true
    }

    func close(animated: Bool) {
        generation &+= 1
        let token = generation
        state = .hidden
        payload = nil
        resignKeyQuietly()
        guard animated, panel.isVisible else { panel.orderOut(nil); return }
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = Motion.micro
            panel.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            guard let self, self.generation == token else { return }
            self.panel.orderOut(nil)
            self.panel.alphaValue = 1
        })
    }

    private func resignKeyQuietly() {
        guard panel.isKeyWindow else { return }
        ignoreResign = true
        // Ordering out hands key focus back to the frontmost (source) app;
        // ordering straight back in keeps the card on screen without key.
        let visible = panel.isVisible
        panel.orderOut(nil)
        if visible && state != .hidden { panel.orderFrontRegardless() }
        ignoreResign = false
    }

    private func handle(_ code: ReviewActionCode) {
        switch (state, code) {
        case (.diff, .copy):
            if let result = payload?.result {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(result, forType: .string)
            }
        case (.diff, .accept):
            // Give key focus back before Rust pastes into the source app.
            state = .hidden
            resignKeyQuietly()
            panel.orderOut(nil)
        case (.diff, .anotherTake):
            resignKeyQuietly()
        case (.diff, _), (.skeleton, .discard), (.skeleton, .dismissed):
            break
        default:
            return
        }
        onAction?(code)
    }

    private func appear(token: UInt64) {
        let reduce = Motion.reduceMotion
        panel.alphaValue = 0
        panel.orderFrontRegardless()
        NSAnimationContext.runAnimationGroup { context in
            context.duration = reduce ? Motion.micro : 0.22
            context.timingFunction = Motion.easeOut
            panel.animator().alphaValue = 1
        }
        if !reduce, let layer = panel.contentView?.layer {
            let lift = CABasicAnimation(keyPath: "transform")
            let below = placement.below
            var start = CATransform3DMakeTranslation(0, below ? 6 : -6, 0)
            start = CATransform3DScale(start, 0.98, 0.98, 1)
            lift.fromValue = start
            lift.toValue = CATransform3DIdentity
            lift.duration = 0.3
            lift.timingFunction = Motion.easeOut
            layer.add(lift, forKey: "appear")
        }
    }

    // MARK: Layout

    private var placement = SelectionGeometry.CardPlacement(origin: .zero, below: true, arrowX: 46)

    private func diffAttributedString(_ payload: DiffPayload, palette: OverlayPalette) -> NSAttributedString {
        let paragraph = NSMutableParagraphStyle()
        let lineHeight = (ReviewCardMetrics.diffFontSize * 1.55).rounded()
        paragraph.minimumLineHeight = lineHeight
        paragraph.maximumLineHeight = lineHeight
        let font = NSFont.systemFont(ofSize: ReviewCardMetrics.diffFontSize)
        let baselineOffset = (lineHeight - font.ascender + font.descender) / 4
        let base: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor.labelColor,
                                                   .paragraphStyle: paragraph, .baselineOffset: baselineOffset]
        let text = NSMutableAttributedString()
        for (index, segment) in payload.segments.enumerated() {
            var attributes = base
            switch segment.op {
            case .equal: break
            case .delete:
                attributes[.foregroundColor] = NSColor.secondaryLabelColor
                attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
                attributes[.strikethroughColor] = NSColor.systemRed
            case .insert:
                attributes[.backgroundColor] = NSColor.systemGreen.withAlphaComponent(palette.dark ? 0.28 : 0.2)
            }
            text.append(NSAttributedString(string: segment.text, attributes: attributes))
            // A word deleted right before its replacement word gets a plain,
            // display-only space so “had” and “Did” don't run together (the
            // reference's `</del> <ins>`). ⌘C copies `result`, not this text.
            if segment.op == .delete, index + 1 < payload.segments.count,
               payload.segments[index + 1].op == .insert,
               segment.text.last.map({ $0.isLetter || $0.isNumber }) == true,
               payload.segments[index + 1].text.first.map({ $0.isLetter || $0.isNumber }) == true {
                text.append(NSAttributedString(string: " ", attributes: base))
            }
        }
        return text
    }

    private func makeDiffView(_ payload: DiffPayload, palette: OverlayPalette) -> (NSView, CGFloat) {
        let storage = NSTextStorage(attributedString: diffAttributedString(payload, palette: palette))
        let layout = RoundedBackgroundLayoutManager()
        storage.addLayoutManager(layout)
        let container = NSTextContainer(size: NSSize(width: ReviewCardMetrics.contentWidth,
                                                     height: .greatestFiniteMagnitude))
        container.lineFragmentPadding = 0
        layout.addTextContainer(container)
        layout.ensureLayout(for: container)
        let textHeight = ceil(layout.usedRect(for: container).height)
        let height = min(textHeight, ReviewCardMetrics.maxDiffHeight)
        let textView = NSTextView(frame: NSRect(x: 0, y: 0, width: ReviewCardMetrics.contentWidth, height: textHeight),
                                  textContainer: container)
        textView.isEditable = false
        textView.isSelectable = false
        textView.drawsBackground = false
        textView.textContainerInset = .zero
        textView.setAccessibilityLabel("Proposed changes")
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: ReviewCardMetrics.contentWidth, height: height))
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = textHeight > height
        scroll.autohidesScrollers = true
        scroll.borderType = .noBorder
        scroll.documentView = textView
        return (scroll, height)
    }

    private func makeSkeleton(palette: OverlayPalette) -> NSView {
        let view = FlippedView(frame: NSRect(x: 0, y: 0, width: ReviewCardMetrics.contentWidth,
                                             height: ReviewCardMetrics.skeletonHeight))
        view.wantsLayer = true
        let line = palette.cg(palette.dark ? NSColor(white: 1, alpha: 0.09) : NSColor(white: 0, alpha: 0.09))
        let strong = palette.cg(palette.dark ? NSColor(white: 1, alpha: 0.18) : NSColor(white: 0, alpha: 0.16))
        for (index, fraction) in [0.92, 0.64].enumerated() {
            let bar = CAGradientLayer()
            bar.frame = CGRect(x: 0, y: 7 + CGFloat(index) * 17,
                               width: (ReviewCardMetrics.contentWidth * fraction).rounded(), height: 10)
            bar.cornerRadius = 4
            bar.colors = [line, strong, line]
            bar.startPoint = CGPoint(x: 0, y: 0.5)
            bar.endPoint = CGPoint(x: 1, y: 0.5)
            if Motion.reduceMotion {
                bar.locations = [0, 0.5, 1]
            } else {
                // A 200%-wide gradient sliding right to left, like `vd-sk`.
                bar.locations = [-1, -0.5, 0]
                let slide = CABasicAnimation(keyPath: "locations")
                slide.fromValue = [1, 1.5, 2]
                slide.toValue = [-1, -0.5, 0]
                slide.duration = Motion.skeletonLoop
                slide.repeatCount = .infinity
                bar.add(slide, forKey: "skeleton")
            }
            view.layer?.addSublayer(bar)
        }
        view.setAccessibilityElement(true)
        view.setAccessibilityRole(.progressIndicator)
        view.setAccessibilityLabel("Writing")
        return view
    }

    private func label(_ string: NSAttributedString) -> NSTextField {
        let field = NSTextField(labelWithAttributedString: string)
        field.usesSingleLineMode = true
        field.maximumNumberOfLines = 1
        field.cell?.wraps = false
        field.lineBreakMode = .byTruncatingTail
        return field
    }

    private func render() {
        let palette = OverlayPalette.resolve(appearance)
        panel.appearance = appearance
        let metrics = ReviewCardMetrics.self
        body.subviews.forEach { $0.removeFromSuperview() }
        var y = metrics.paddingTop
        let header = NSMutableAttributedString(string: title, attributes: [
            .font: NSFont.systemFont(ofSize: 11.5, weight: .semibold), .foregroundColor: NSColor.labelColor])
        if !model.isEmpty {
            header.append(NSAttributedString(string: " · \(model)", attributes: [
                .font: NSFont.systemFont(ofSize: 11.5), .foregroundColor: NSColor.secondaryLabelColor]))
        }
        let headerLeft = label(header)
        let rightText = payload?.deltaLabel ?? "esc to cancel"
        let headerRight = label(NSAttributedString(string: rightText, attributes: [
            .font: NSFont.monospacedDigitSystemFont(ofSize: 11.5, weight: .regular),
            .foregroundColor: NSColor.secondaryLabelColor]))
        headerRight.alignment = .right
        let rightWidth = ceil(headerRight.attributedStringValue.size().width) + 2
        headerRight.frame = NSRect(x: metrics.paddingX + metrics.contentWidth - rightWidth, y: y,
                                   width: rightWidth, height: metrics.headerHeight)
        headerLeft.frame = NSRect(x: metrics.paddingX, y: y,
                                  width: metrics.contentWidth - rightWidth - 12, height: metrics.headerHeight)
        body.addSubview(headerLeft)
        body.addSubview(headerRight)
        y += metrics.headerHeight + metrics.headerGap
        if let payload {
            let (diff, height) = makeDiffView(payload, palette: palette)
            diff.frame.origin = NSPoint(x: metrics.paddingX, y: y)
            body.addSubview(diff)
            y += height
        } else {
            let skeleton = makeSkeleton(palette: palette)
            skeleton.frame.origin = NSPoint(x: metrics.paddingX, y: y)
            body.addSubview(skeleton)
            y += metrics.skeletonHeight
        }
        y += 10
        let rule = NSBox(frame: NSRect(x: metrics.paddingX, y: y, width: metrics.contentWidth, height: 1))
        rule.boxType = .custom
        rule.borderWidth = 0
        rule.fillColor = palette.dark ? NSColor(white: 1, alpha: 0.09) : NSColor(white: 0, alpha: 0.09)
        rule.frame.size.height = 0.5
        body.addSubview(rule)
        y += 8
        let actions: [(String, String, ReviewActionCode)] = [
            ("↩", "Replace", .accept), ("⇥", "Another take", .anotherTake), ("⌘C", "Copy", .copy),
        ]
        var x = metrics.paddingX
        let enabled = payload != nil
        for (keys, text, code) in actions {
            let item = FooterAction(keys: keys, title: text, code: code, palette: palette)
            item.onClick = { [weak self] code in self?.handle(code) }
            item.alphaValue = enabled ? 1 : 0.45
            let size = item.fittingSize
            item.frame = NSRect(x: x, y: y, width: ceil(size.width), height: 20)
            body.addSubview(item)
            x += ceil(size.width) + 14
        }
        let discard = FooterAction(keys: "esc", title: enabled ? "Discard" : "Cancel", code: .discard, palette: palette)
        discard.onClick = { [weak self] code in self?.handle(code) }
        let discardSize = discard.fittingSize
        discard.frame = NSRect(x: metrics.paddingX + metrics.contentWidth - ceil(discardSize.width), y: y,
                               width: ceil(discardSize.width), height: 20)
        body.addSubview(discard)
        y += 20 + metrics.paddingBottom
        layoutPanel(cardHeight: y, palette: palette)
    }

    private func layoutPanel(cardHeight: CGFloat, palette: OverlayPalette) {
        let metrics = ReviewCardMetrics.self
        let size = NSSize(width: metrics.width, height: cardHeight + metrics.arrowHeight)
        let visible = InkSweep.visibleFrame(for: [anchor])
        placement = SelectionGeometry.cardPlacement(size: size, anchor: anchor, pointX: pointX, visible: visible)
        panel.setFrame(NSRect(origin: placement.origin, size: size), display: false)
        let bodyY: CGFloat = placement.below ? metrics.arrowHeight : 0
        let bounds = NSRect(origin: .zero, size: size)
        panel.contentView?.frame = bounds
        effect.frame = bounds
        body.frame = NSRect(x: 0, y: bodyY, width: metrics.width, height: cardHeight)
        // Shape: rounded card plus an arrow toward the selection (flipped coordinates).
        let path = CGMutablePath()
        let card = CGRect(x: 0, y: bodyY, width: metrics.width, height: cardHeight)
        path.addRoundedRect(in: card, cornerWidth: metrics.radius, cornerHeight: metrics.radius)
        let half = metrics.arrowWidth / 2
        let arrow = CGMutablePath()
        if placement.below {
            arrow.move(to: CGPoint(x: placement.arrowX - half, y: bodyY + 0.5))
            arrow.addLine(to: CGPoint(x: placement.arrowX, y: 0))
            arrow.addLine(to: CGPoint(x: placement.arrowX + half, y: bodyY + 0.5))
        } else {
            arrow.move(to: CGPoint(x: placement.arrowX - half, y: cardHeight - 0.5))
            arrow.addLine(to: CGPoint(x: placement.arrowX, y: size.height))
            arrow.addLine(to: CGPoint(x: placement.arrowX + half, y: cardHeight - 0.5))
        }
        arrow.closeSubpath()
        path.addPath(arrow)
        let maskPath = path
        effect.maskImage = NSImage(size: size, flipped: true) { _ in
            guard let context = NSGraphicsContext.current?.cgContext else { return false }
            context.addPath(maskPath)
            context.setFillColor(NSColor.black.cgColor)
            context.fillPath()
            return true
        }
        // Hairline outline that follows the card and its arrow.
        let outline = CGMutablePath()
        let inset = card.insetBy(dx: 0.25, dy: 0.25)
        let r = metrics.radius
        if placement.below {
            outline.move(to: CGPoint(x: inset.minX + r, y: inset.minY))
            outline.addLine(to: CGPoint(x: placement.arrowX - half, y: inset.minY))
            outline.addLine(to: CGPoint(x: placement.arrowX, y: 0.25))
            outline.addLine(to: CGPoint(x: placement.arrowX + half, y: inset.minY))
            outline.addArc(tangent1End: CGPoint(x: inset.maxX, y: inset.minY), tangent2End: CGPoint(x: inset.maxX, y: inset.maxY), radius: r)
            outline.addArc(tangent1End: CGPoint(x: inset.maxX, y: inset.maxY), tangent2End: CGPoint(x: inset.minX, y: inset.maxY), radius: r)
            outline.addArc(tangent1End: CGPoint(x: inset.minX, y: inset.maxY), tangent2End: CGPoint(x: inset.minX, y: inset.minY), radius: r)
            outline.addArc(tangent1End: CGPoint(x: inset.minX, y: inset.minY), tangent2End: CGPoint(x: inset.maxX, y: inset.minY), radius: r)
        } else {
            outline.move(to: CGPoint(x: inset.minX + r, y: inset.minY))
            outline.addArc(tangent1End: CGPoint(x: inset.maxX, y: inset.minY), tangent2End: CGPoint(x: inset.maxX, y: inset.maxY), radius: r)
            outline.addArc(tangent1End: CGPoint(x: inset.maxX, y: inset.maxY), tangent2End: CGPoint(x: inset.minX, y: inset.maxY), radius: r)
            outline.addLine(to: CGPoint(x: placement.arrowX + half, y: inset.maxY))
            outline.addLine(to: CGPoint(x: placement.arrowX, y: size.height - 0.25))
            outline.addLine(to: CGPoint(x: placement.arrowX - half, y: inset.maxY))
            outline.addArc(tangent1End: CGPoint(x: inset.minX, y: inset.maxY), tangent2End: CGPoint(x: inset.minX, y: inset.minY), radius: r)
            outline.addArc(tangent1End: CGPoint(x: inset.minX, y: inset.minY), tangent2End: CGPoint(x: inset.maxX, y: inset.minY), radius: r)
        }
        outline.closeSubpath()
        wash.frame = bounds
        wash.shape = path
        wash.color = (palette.dark
            ? NSColor(srgbRed: 0.173, green: 0.173, blue: 0.184, alpha: 0.55)
            : NSColor(white: 1, alpha: 0.86))
        border.frame = bounds
        border.path = outline
        border.lineWidth = 0.5
        border.strokeColor = palette.cg(palette.dark ? NSColor(white: 1, alpha: 0.18) : NSColor(white: 0, alpha: 0.16))
        // Keep the flipped border layer aligned with the flipped content view.
        border.isGeometryFlipped = false
        panel.invalidateShadow()
    }
}
