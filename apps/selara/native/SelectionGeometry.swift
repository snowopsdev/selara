// Selection geometry for the moment-of-use overlays: per-line Accessibility
// bounds of the selected text, a plausibility gate that decides between the
// Ink Sweep and the orb fallback, and pure placement helpers. Main thread only.
import AppKit
import ApplicationServices

/// Why the Ink Sweep falls back to the orb. Kept explicit for tests and logs.
enum SweepFallbackReason: Equatable {
    case noBounds
    case webArea
    case tooManyLines
    case notFinite
    case emptyRect
    case tooTall
    case largerThanElement
    case offScreen
}

enum SweepGeometry: Equatable {
    /// Line rectangles in AppKit screen points, top line first.
    case lines([NSRect])
    case fallback(SweepFallbackReason)

    var lines: [NSRect]? {
        if case .lines(let lines) = self { return lines }
        return nil
    }
}

/// What `captureAnchor` learned about the source selection.
struct SelectionCapture {
    var pid: pid_t
    var range: CFRange?
    var element: NSRect?
    var isWebArea: Bool
    var geometry: SweepGeometry
}

enum SelectionGeometry {
    static let maxLines = 12
    /// No single text line is this tall; taller rects are whole-element misreports.
    static let maxLineHeight: CGFloat = 160
    /// AX frames are rounded differently from text bounds.
    static let elementTolerance: CGFloat = 4
    /// Total AX budget for one capture; a slow app gets the orb, not a hang.
    static let budget: TimeInterval = 0.15

    // MARK: Pure decisions

    /// Accessibility uses a top-left origin on the primary display; AppKit
    /// uses a bottom-left origin on the same display.
    static func appKitRect(_ ax: CGRect, primaryMaxY: CGFloat) -> NSRect {
        NSRect(x: ax.minX, y: primaryMaxY - ax.maxY, width: ax.width, height: ax.height)
    }

    /// Decide whether per-line bounds can carry the Ink Sweep. `lines` and
    /// `element` are AppKit rects; `lineCount` is the number of text lines the
    /// selection spans (it can exceed `lines.count` when some were skipped).
    static func decide(lines: [NSRect], lineCount: Int, element: NSRect?,
                       screens: [NSRect], isWebArea: Bool) -> SweepGeometry {
        if isWebArea { return .fallback(.webArea) }
        if lineCount > maxLines || lines.count > maxLines { return .fallback(.tooManyLines) }
        if lines.isEmpty { return .fallback(.noBounds) }
        let values = lines.flatMap { [$0.minX, $0.minY, $0.width, $0.height] }
        if !values.allSatisfy({ $0.isFinite }) { return .fallback(.notFinite) }
        if lines.contains(where: { $0.size.height <= 0 || $0.size.width < 0 }) { return .fallback(.emptyRect) }
        // Zero-width fragments are line breaks inside the selection; skip them.
        let visible = lines.filter { $0.width >= 1 }
        if visible.isEmpty { return .fallback(.emptyRect) }
        if visible.contains(where: { $0.height > maxLineHeight }) { return .fallback(.tooTall) }
        let union = visible.dropFirst().reduce(visible[0]) { $0.union($1) }
        if let element, element.width > 0, element.height > 0 {
            let tolerance = elementTolerance
            if union.width > element.width + tolerance || union.height > element.height + tolerance
                || !union.intersects(element.insetBy(dx: -tolerance, dy: -tolerance)) {
                return .fallback(.largerThanElement)
            }
        }
        let onScreen = visible.filter { line in screens.contains { $0.intersects(line) } }
        if onScreen.isEmpty { return .fallback(.offScreen) }
        return .lines(onScreen.sorted { $0.maxY == $1.maxY ? $0.minX < $1.minX : $0.maxY > $1.maxY })
    }

    static func union(_ rects: [NSRect]) -> NSRect? {
        guard let first = rects.first else { return nil }
        return rects.dropFirst().reduce(first) { $0.union($1) }
    }

    /// Working chip: 4 pt above the first line, left-aligned with it; below
    /// the last line when the top of the display is in the way.
    static func chipOrigin(size: NSSize, lines: [NSRect], visible: NSRect) -> NSPoint {
        guard let first = lines.first, let last = lines.last else { return visible.origin }
        let gap: CGFloat = 4
        var origin = NSPoint(x: first.minX, y: first.maxY + gap)
        if origin.y + size.height > visible.maxY { origin.y = last.minY - gap - size.height }
        return clamp(origin, size: size, in: visible)
    }

    /// “Replaced · ⌘Z to undo”: below the end of the last line, ending where
    /// the replaced text ends; above the first line near the display bottom.
    static func hintOrigin(size: NSSize, lines: [NSRect], visible: NSRect) -> NSPoint {
        guard let first = lines.first, let last = lines.last else { return visible.origin }
        let gap: CGFloat = 6
        var origin = NSPoint(x: max(last.minX, last.maxX - size.width), y: last.minY - gap - size.height)
        if origin.y < visible.minY { origin.y = first.maxY + gap }
        return clamp(origin, size: size, in: visible)
    }

    struct CardPlacement: Equatable {
        var origin: NSPoint
        /// True when the card sits below the selection and its arrow points up.
        var below: Bool
        /// Arrow tip x, relative to the card's left edge.
        var arrowX: CGFloat
    }

    /// Review card: below the selection with its arrow pointing at the first
    /// line's start; above the selection when there is no room below.
    static func cardPlacement(size: NSSize, anchor: NSRect, pointX: CGFloat,
                              visible: NSRect, gap: CGFloat = 4) -> CardPlacement {
        let margin: CGFloat = 8
        var below = true
        var y = anchor.minY - gap - size.height
        if y < visible.minY + margin {
            let above = anchor.maxY + gap
            if above + size.height <= visible.maxY - margin {
                below = false
                y = above
            } else {
                y = visible.minY + margin
            }
        }
        let arrowInset: CGFloat = 46
        let x = max(visible.minX + margin, min(pointX - arrowInset, visible.maxX - margin - size.width))
        let arrowX = max(20, min(pointX - x, size.width - 20))
        return CardPlacement(origin: NSPoint(x: x, y: y), below: below, arrowX: arrowX)
    }

    static func clamp(_ origin: NSPoint, size: NSSize, in visible: NSRect) -> NSPoint {
        NSPoint(x: max(visible.minX, min(origin.x, visible.maxX - size.width)),
                y: max(visible.minY, min(origin.y, visible.maxY - size.height)))
    }

    // MARK: Accessibility reads

    static var primaryMaxY: CGFloat { NSScreen.screens.first?.frame.maxY ?? 0 }
    static var screenFrames: [NSRect] { NSScreen.screens.map(\.frame) }

    static func focusedElement(pid: pid_t) -> AXUIElement? {
        guard pid > 0 else { return nil }
        let app = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(app, 0.06)
        var focused: CFTypeRef?
        guard AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute as CFString, &focused) == .success,
              let focused, CFGetTypeID(focused) == AXUIElementGetTypeID() else { return nil }
        let element = unsafeBitCast(focused, to: AXUIElement.self)
        AXUIElementSetMessagingTimeout(element, 0.06)
        return element
    }

    static func frame(of element: AXUIElement) -> CGRect? {
        var position: CFTypeRef?
        var size: CFTypeRef?
        var origin = CGPoint.zero
        var dimensions = CGSize.zero
        guard AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &position) == .success,
              AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &size) == .success,
              let position, let size,
              CFGetTypeID(position) == AXValueGetTypeID(), CFGetTypeID(size) == AXValueGetTypeID(),
              AXValueGetValue(unsafeBitCast(position, to: AXValue.self), .cgPoint, &origin),
              AXValueGetValue(unsafeBitCast(size, to: AXValue.self), .cgSize, &dimensions) else { return nil }
        return CGRect(origin: origin, size: dimensions)
    }

    static func role(of element: AXUIElement) -> String? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &value) == .success else { return nil }
        return value as? String
    }

    static func selectedRange(of element: AXUIElement) -> CFRange? {
        var value: CFTypeRef?
        var range = CFRange()
        guard AXUIElementCopyAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, &value) == .success,
              let value, CFGetTypeID(value) == AXValueGetTypeID(),
              AXValueGetValue(unsafeBitCast(value, to: AXValue.self), .cfRange, &range),
              range.location >= 0, range.length >= 0 else { return nil }
        return range
    }

    static func bounds(of element: AXUIElement, range: CFRange) -> CGRect? {
        var range = range
        guard let parameter = AXValueCreate(.cfRange, &range) else { return nil }
        var value: CFTypeRef?
        var rect = CGRect.zero
        guard AXUIElementCopyParameterizedAttributeValue(element, kAXBoundsForRangeParameterizedAttribute as CFString,
                                                         parameter, &value) == .success,
              let value, CFGetTypeID(value) == AXValueGetTypeID(),
              AXValueGetValue(unsafeBitCast(value, to: AXValue.self), .cgRect, &rect) else { return nil }
        return rect
    }

    static func line(of element: AXUIElement, index: Int) -> Int? {
        var value: CFTypeRef?
        let parameter = NSNumber(value: index) as CFNumber
        guard AXUIElementCopyParameterizedAttributeValue(element, kAXLineForIndexParameterizedAttribute as CFString,
                                                         parameter, &value) == .success,
              let number = value as? NSNumber else { return nil }
        return number.intValue
    }

    static func range(of element: AXUIElement, line: Int) -> CFRange? {
        var value: CFTypeRef?
        var range = CFRange()
        let parameter = NSNumber(value: line) as CFNumber
        guard AXUIElementCopyParameterizedAttributeValue(element, kAXRangeForLineParameterizedAttribute as CFString,
                                                         parameter, &value) == .success,
              let value, CFGetTypeID(value) == AXValueGetTypeID(),
              AXValueGetValue(unsafeBitCast(value, to: AXValue.self), .cfRange, &range) else { return nil }
        return range
    }

    /// Per-line AX rects (AX coordinates) for `range`, and the number of lines
    /// it spans. Without line attributes, the whole range is one rect.
    static func lineRects(of element: AXUIElement, range: CFRange) -> (rects: [CGRect], lineCount: Int)? {
        guard range.length > 0 else { return nil }
        let started = ProcessInfo.processInfo.systemUptime
        func overBudget() -> Bool { ProcessInfo.processInfo.systemUptime - started > budget }
        let end = range.location + range.length
        if let firstLine = line(of: element, index: range.location),
           let lastLine = line(of: element, index: end - 1), lastLine >= firstLine {
            let count = lastLine - firstLine + 1
            if count > maxLines { return ([], count) }
            var rects: [CGRect] = []
            for number in firstLine...lastLine {
                if overBudget() { return nil }
                guard let lineRange = self.range(of: element, line: number) else { continue }
                let start = max(range.location, lineRange.location)
                let stop = min(end, lineRange.location + lineRange.length)
                guard stop > start,
                      let rect = bounds(of: element, range: CFRange(location: start, length: stop - start)) else { continue }
                rects.append(rect)
            }
            if !rects.isEmpty { return (rects, count) }
        }
        if overBudget() { return nil }
        guard let rect = bounds(of: element, range: range) else { return nil }
        return ([rect], 1)
    }

    /// Read the focused element's selection geometry for the Ink Sweep.
    static func capture(pid: pid_t, element: AXUIElement) -> SelectionCapture {
        let top = primaryMaxY
        let elementFrame = frame(of: element).map { appKitRect($0, primaryMaxY: top) }
        let isWebArea = role(of: element) == "AXWebArea"
        let range = selectedRange(of: element)
        var geometry = SweepGeometry.fallback(.noBounds)
        if let range, !isWebArea, let measured = lineRects(of: element, range: range) {
            geometry = decide(lines: measured.rects.map { appKitRect($0, primaryMaxY: top) },
                              lineCount: measured.lineCount, element: elementFrame,
                              screens: screenFrames, isWebArea: false)
        } else if isWebArea {
            geometry = .fallback(.webArea)
        }
        return SelectionCapture(pid: pid, range: range, element: elementFrame,
                                isWebArea: isWebArea, geometry: geometry)
    }

    /// Bounds of the replaced text after a verified paste, for the afterglow.
    static func replacedLines(pid: pid_t, location: Int, length: Int, element: NSRect?) -> [NSRect]? {
        guard location >= 0, length > 0, let focused = focusedElement(pid: pid),
              let measured = lineRects(of: focused, range: CFRange(location: location, length: length)) else { return nil }
        let top = primaryMaxY
        return decide(lines: measured.rects.map { appKitRect($0, primaryMaxY: top) },
                      lineCount: measured.lineCount, element: element,
                      screens: screenFrames, isWebArea: role(of: focused) == "AXWebArea").lines
    }
}
