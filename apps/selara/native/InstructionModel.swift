// Pure logic behind the custom-instruction popover (design direction 2B):
// intent chips, the sentence they compile into, ↑/↓ recall stepping, popover
// placement, and the Swift → Rust event encoding. No AppKit windows here, so
// `Tests/Instruction/main.swift` can exercise all of it headlessly.
import CoreGraphics
import Foundation

/// Mirrors the Settings motion tokens (design-explorations/implementation-contract.md).
/// Named for this stream so it cannot collide with the overlay's `Motion`.
enum InstructionMotion {
    static let micro: TimeInterval = 0.12   // --motion-micro: chips, receipts
    static let panel: TimeInterval = 0.24   // --motion-panel: popover appear
    static let hold: TimeInterval = 1.8     // --motion-hold: "Saved as command"
    static let reducedFade: TimeInterval = 0.12  // reduce motion: opacity only
    /// --ease-out: cubic-bezier(.2,.8,.2,1)
    static let easeOut: (Float, Float, Float, Float) = (0.2, 0.8, 0.2, 1)
    /// Popover scale keyframes approximating --ease-spring
    /// cubic-bezier(.2,.9,.25,1.15): a small overshoot, then settle.
    static let appearScale: [CGFloat] = [0.94, 1.012, 1]
    static let appearKeyTimes: [Double] = [0, 0.72, 1]
}

/// The fixed chip vocabulary, in display (and number-key) order.
enum InstructionChip: Int, CaseIterable {
    case shorter = 1, warmer, formal, direct, bullets, fixOnly

    var title: String {
        switch self {
        case .shorter: return "Shorter"
        case .warmer: return "Warmer"
        case .formal: return "Formal"
        case .direct: return "Direct"
        case .bullets: return "Bullets"
        case .fixOnly: return "Fix only"
        }
    }

    /// Tone and length chips: the "Make it …" list.
    var adjective: String? {
        switch self {
        case .shorter: return "shorter"
        case .warmer: return "warmer"
        case .formal: return "more formal"
        case .direct: return "more direct"
        case .bullets, .fixOnly: return nil
        }
    }

    /// Number key 1–6 (bare while the detail is empty, or with ⌘ always).
    init?(key: String) {
        guard key.count == 1, let digit = Int(key), let chip = InstructionChip(rawValue: digit) else {
            return nil
        }
        self = chip
    }
}

/// Selected chips in the order they were turned on. "Fix only" excludes the
/// tone/length chips (it would contradict them); Bullets combines with either.
struct InstructionChips: Equatable {
    private(set) var order: [InstructionChip] = []

    func contains(_ chip: InstructionChip) -> Bool { order.contains(chip) }
    var isEmpty: Bool { order.isEmpty }

    mutating func toggle(_ chip: InstructionChip) {
        if let index = order.firstIndex(of: chip) {
            order.remove(at: index)
            return
        }
        if chip == .fixOnly {
            order.removeAll { $0.adjective != nil }
        } else if chip.adjective != nil {
            order.removeAll { $0 == .fixOnly }
        }
        order.append(chip)
    }

    mutating func removeLast() { _ = order.popLast() }
    mutating func removeAll() { order.removeAll() }
}

/// `a`, `a and b`, `a, b and c` (the reference's list style, no Oxford comma).
func instructionList(_ items: [String]) -> String {
    switch items.count {
    case 0: return ""
    case 1: return items[0]
    default: return items.dropLast().joined(separator: ", ") + " and " + items[items.count - 1]
    }
}

/// The sentence the chips stand for. Empty when no chip is on.
///   [shorter, warmer]            → "Make it shorter and warmer."
///   [direct, bullets, shorter]   → "Make it more direct and shorter, as bullet points."
///   [bullets]                    → "Rewrite it as bullet points."
///   [fixOnly]                    → "Fix only spelling and grammar."
func compileInstructionChips(_ chips: InstructionChips) -> String {
    let adjectives = chips.order.compactMap(\.adjective)
    let bullets = chips.contains(.bullets) ? "as bullet points" : nil
    if chips.contains(.fixOnly) {
        return "Fix only spelling and grammar" + (bullets.map { ", " + $0 } ?? "") + "."
    }
    if !adjectives.isEmpty {
        return "Make it " + instructionList(adjectives) + (bullets.map { ", " + $0 } ?? "") + "."
    }
    if let bullets { return "Rewrite it " + bullets + "." }
    return ""
}

/// The field's leading text: the compiled sentence plus the space that the
/// user's detail follows. Empty when no chip is on.
func instructionPrefix(_ chips: InstructionChips) -> String {
    let compiled = compileInstructionChips(chips)
    return compiled.isEmpty ? "" : compiled + " "
}

/// Final instruction sent to Rust: compiled sentence + space + typed detail.
func composeInstruction(compiled: String, detail: String) -> String {
    [compiled.trimmingCharacters(in: .whitespacesAndNewlines),
     detail.trimmingCharacters(in: .whitespacesAndNewlines)]
        .filter { !$0.isEmpty }
        .joined(separator: " ")
}

/// ↑ (`delta > 0`) walks older, ↓ newer through Rust's `instruction_history`.
/// `nil` is the empty field; ↓ from the newest entry returns to it.
func instructionHistoryStep(_ current: Int?, count: Int, delta: Int) -> Int? {
    guard count > 0 else { return nil }
    switch (current, delta.signum()) {
    case (nil, 1): return 0
    case (let i?, 1): return min(i + 1, count - 1)
    case (0?, -1): return nil
    case (let i?, -1): return i - 1
    default: return current
    }
}

/// Meta row, right side: `Mail · 1,234 chars`.
func instructionSelectionSummary(app: String?, chars: UInt64, locale: Locale = .current) -> String {
    let formatter = NumberFormatter()
    formatter.locale = locale
    formatter.numberStyle = .decimal
    let count = formatter.string(from: NSNumber(value: chars)) ?? String(chars)
    let size = "\(count) \(chars == 1 ? "char" : "chars")"
    guard let app = app?.trimmingCharacters(in: .whitespacesAndNewlines), !app.isEmpty else {
        return size
    }
    return "\(app) · \(size)"
}

// MARK: - Placement

/// Arrow geometry shared by layout, drawing, and the placement math.
enum InstructionArrow {
    static let height: CGFloat = 8
    static let halfBase: CGFloat = 9
    /// Preferred distance of the arrow tip from the popover's left edge (the
    /// reference anchors it near the start of the selection).
    static let preferredOffset: CGFloat = 64
    /// Keep the arrow clear of the 14 pt corners.
    static let edgeInset: CGFloat = 14 + 9 + 4
    /// Space between the selection and the arrow tip.
    static let gap: CGFloat = 4
    /// Keep the popover this far from the visible frame's edges.
    static let margin: CGFloat = 6
}

enum InstructionArrowEdge: Equatable {
    /// Popover below the selection, arrow on its top edge pointing up.
    case top
    /// Popover above the selection, arrow on its bottom edge pointing down.
    case bottom
}

struct InstructionPlacement: Equatable {
    /// Panel frame in AppKit screen points (bottom-left origin), arrow included.
    var frame: CGRect
    var edge: InstructionArrowEdge
    /// Arrow tip x, measured from the panel's left edge.
    var arrowX: CGFloat
}

/// Where the popover (`body` size, without the arrow) goes for a selection
/// `anchor` inside `visible` (both AppKit screen points). Below the selection
/// when it fits, above when it doesn't, otherwise on the roomier side clamped
/// into the visible frame. A zero-height anchor is the pointer fallback and
/// gets the I-beam's height so the arrow clears the cursor.
func instructionPopoverPlacement(anchor rawAnchor: CGRect, body: CGSize, visible: CGRect) -> InstructionPlacement {
    let anchor = rawAnchor.height > 0 ? rawAnchor : rawAnchor.insetBy(dx: 0, dy: -10)
    let size = CGSize(width: body.width, height: body.height + InstructionArrow.height)
    let margin = InstructionArrow.margin

    // Point at the start of a long selection, the middle of a short one.
    let targetX = anchor.width > 0 ? min(anchor.midX, anchor.minX + InstructionArrow.preferredOffset) : anchor.minX
    let minX = visible.minX + margin
    let maxX = visible.maxX - margin - size.width
    let x = maxX < minX ? visible.minX : min(max(targetX - InstructionArrow.preferredOffset, minX), maxX)
    let inset = InstructionArrow.edgeInset
    let arrowX = min(max(targetX - x, inset), max(inset, size.width - inset))

    let belowY = anchor.minY - InstructionArrow.gap - size.height
    let aboveY = anchor.maxY + InstructionArrow.gap
    let fitsBelow = belowY >= visible.minY + margin
    let fitsAbove = aboveY + size.height <= visible.maxY - margin
    let edge: InstructionArrowEdge
    var y: CGFloat
    if fitsBelow {
        edge = .top; y = belowY
    } else if fitsAbove {
        edge = .bottom; y = aboveY
    } else {
        // Neither side fits (a very tall selection): take the roomier side
        // and let the popover overlap the selection rather than leave the screen.
        let roomBelow = anchor.minY - visible.minY
        let roomAbove = visible.maxY - anchor.maxY
        edge = roomBelow >= roomAbove ? .top : .bottom
        y = edge == .top ? belowY : aboveY
    }
    let minY = visible.minY + margin
    let maxY = visible.maxY - margin - size.height
    y = maxY < minY ? visible.minY : min(max(y, minY), maxY)
    return InstructionPlacement(frame: CGRect(x: x, y: y, width: size.width, height: size.height),
                                edge: edge, arrowX: arrowX)
}

/// The bubble outline (rounded body plus arrow) for a panel of `size`, in
/// view coordinates with a bottom-left origin.
func instructionBubblePath(size: CGSize, edge: InstructionArrowEdge, arrowX: CGFloat,
                           radius: CGFloat = 14) -> CGPath {
    let h = InstructionArrow.height, half = InstructionArrow.halfBase
    let bodyMinY: CGFloat = edge == .bottom ? h : 0
    let bodyMaxY: CGFloat = edge == .top ? size.height - h : size.height
    let w = size.width
    let path = CGMutablePath()
    path.move(to: CGPoint(x: 0, y: (bodyMinY + bodyMaxY) / 2))
    // Clockwise in a y-up space: left edge up, across the top, down, back.
    path.addArc(tangent1End: CGPoint(x: 0, y: bodyMaxY), tangent2End: CGPoint(x: w, y: bodyMaxY), radius: radius)
    if edge == .top {
        path.addArc(tangent1End: CGPoint(x: arrowX - half, y: bodyMaxY), tangent2End: CGPoint(x: arrowX, y: size.height), radius: 1.5)
        path.addArc(tangent1End: CGPoint(x: arrowX, y: size.height), tangent2End: CGPoint(x: arrowX + half, y: bodyMaxY), radius: 2.5)
        path.addArc(tangent1End: CGPoint(x: arrowX + half, y: bodyMaxY), tangent2End: CGPoint(x: w, y: bodyMaxY), radius: 1.5)
    }
    path.addArc(tangent1End: CGPoint(x: w, y: bodyMaxY), tangent2End: CGPoint(x: w, y: bodyMinY), radius: radius)
    path.addArc(tangent1End: CGPoint(x: w, y: bodyMinY), tangent2End: CGPoint(x: 0, y: bodyMinY), radius: radius)
    if edge == .bottom {
        path.addArc(tangent1End: CGPoint(x: arrowX + half, y: bodyMinY), tangent2End: CGPoint(x: arrowX, y: 0), radius: 1.5)
        path.addArc(tangent1End: CGPoint(x: arrowX, y: 0), tangent2End: CGPoint(x: arrowX - half, y: bodyMinY), radius: 2.5)
        path.addArc(tangent1End: CGPoint(x: arrowX - half, y: bodyMinY), tangent2End: CGPoint(x: 0, y: bodyMinY), radius: 1.5)
    }
    path.addArc(tangent1End: CGPoint(x: 0, y: bodyMinY), tangent2End: CGPoint(x: 0, y: bodyMaxY), radius: radius)
    path.closeSubpath()
    return path
}

// MARK: - Events

/// What the popover asks Rust to do. Encoded as `kind` or `kind\ntext`;
/// decoded by `instruction::decode_event` in Rust.
enum InstructionEvent: Equatable {
    case submit(String)
    case save(String)
    /// Esc: close and hand focus back to the source app.
    case cancel
    /// The popover lost key focus (a click elsewhere): close, don't refocus.
    case dismiss

    var encoded: String {
        switch self {
        case .submit(let text): return "submit\n" + text
        case .save(let text): return "save\n" + text
        case .cancel: return "cancel"
        case .dismiss: return "dismiss"
        }
    }
}
