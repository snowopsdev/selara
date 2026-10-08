// Instruction popover regressions: compile with ../../Instruction*.swift.
import AppKit

var failures = 0
func expect(_ condition: @autoclosure () -> Bool, _ message: String, line: UInt = #line) {
    guard condition() else {
        fputs("FAIL at line \(line): \(message)\n", stderr)
        failures += 1
        return
    }
}
func chips(_ list: InstructionChip...) -> InstructionChips {
    var set = InstructionChips()
    for chip in list { set.toggle(chip) }
    return set
}

// MARK: Chip compilation

expect(compileInstructionChips(chips()) == "", "no chips compile to nothing")
expect(compileInstructionChips(chips(.shorter)) == "Make it shorter.", "single adjective")
expect(compileInstructionChips(chips(.shorter, .warmer)) == "Make it shorter and warmer.", "reference example")
expect(compileInstructionChips(chips(.warmer, .shorter)) == "Make it warmer and shorter.", "click order is kept")
expect(compileInstructionChips(chips(.direct, .formal, .shorter)) == "Make it more direct, more formal and shorter.",
       "three adjectives use the reference list style")
expect(compileInstructionChips(chips(.shorter, .bullets)) == "Make it shorter, as bullet points.", "bullets qualify the list")
expect(compileInstructionChips(chips(.bullets, .shorter)) == "Make it shorter, as bullet points.", "bullets always trail")
expect(compileInstructionChips(chips(.bullets)) == "Rewrite it as bullet points.", "bullets alone")
expect(compileInstructionChips(chips(.fixOnly)) == "Fix only spelling and grammar.", "fix only")
expect(compileInstructionChips(chips(.fixOnly, .bullets)) == "Fix only spelling and grammar, as bullet points.", "fix only + bullets")
expect(chips(.shorter, .warmer, .fixOnly).order == [.fixOnly], "fix only clears tone and length chips")
expect(chips(.fixOnly, .bullets, .direct).order == [.bullets, .direct], "a tone chip clears fix only, bullets stay")
expect(chips(.shorter, .shorter).isEmpty, "toggling twice turns a chip off")
var trimmed = chips(.shorter, .warmer)
trimmed.removeLast()
expect(trimmed.order == [.shorter], "backspace removes the most recent chip")
expect(instructionPrefix(chips(.shorter)) == "Make it shorter. ", "prefix ends with the detail's space")
expect(instructionPrefix(chips()) == "", "no prefix without chips")
expect(composeInstruction(compiled: "Make it shorter.", detail: "  Keep the date. ") == "Make it shorter. Keep the date.",
       "compiled + space + detail")
expect(composeInstruction(compiled: "", detail: " Translate to French \n") == "Translate to French", "detail only")
expect(composeInstruction(compiled: "Make it warmer.", detail: "") == "Make it warmer.", "chips only")
expect(InstructionChip(key: "1") == .shorter && InstructionChip(key: "6") == .fixOnly, "number keys map 1–6")
expect(InstructionChip(key: "7") == nil && InstructionChip(key: "0") == nil && InstructionChip(key: "a") == nil,
       "other keys are not chips")

// MARK: History stepping (the rule the egui dialog's history_step used)

expect(instructionHistoryStep(nil, count: 0, delta: 1) == nil, "empty history")
expect(instructionHistoryStep(nil, count: 3, delta: 1) == 0, "↑ from empty recalls newest")
expect(instructionHistoryStep(0, count: 3, delta: 1) == 1, "↑ walks older")
expect(instructionHistoryStep(2, count: 3, delta: 1) == 2, "↑ stays on oldest")
expect(instructionHistoryStep(2, count: 3, delta: -1) == 1, "↓ walks newer")
expect(instructionHistoryStep(0, count: 3, delta: -1) == nil, "↓ from newest returns to empty")
expect(instructionHistoryStep(nil, count: 3, delta: -1) == nil, "↓ from empty stays empty")

// MARK: Meta summary

let us = Locale(identifier: "en_US")
expect(instructionSelectionSummary(app: "Mail", chars: 129, locale: us) == "Mail · 129 chars", "reference summary")
expect(instructionSelectionSummary(app: "Notes", chars: 12_345, locale: us) == "Notes · 12,345 chars", "thousands separator")
expect(instructionSelectionSummary(app: nil, chars: 1, locale: us) == "1 char", "singular, no app")
expect(instructionSelectionSummary(app: "  ", chars: 2, locale: us) == "2 chars", "blank app name is omitted")

// MARK: Placement (AppKit points, bottom-left origin)

let visible = CGRect(x: 0, y: 0, width: 1440, height: 875)
let body = CGSize(width: 470, height: 130)
let h = body.height + InstructionArrow.height
do {
    let selection = CGRect(x: 300, y: 600, width: 400, height: 40)
    let p = instructionPopoverPlacement(anchor: selection, body: body, visible: visible)
    expect(p.edge == .top, "room below: popover sits under the selection")
    expect(p.frame.maxY == selection.minY - InstructionArrow.gap, "arrow tip just below the selection")
    expect(p.frame.height == h, "frame includes the arrow")
    expect(p.frame.minX + p.arrowX == selection.minX + InstructionArrow.preferredOffset, "long selection: arrow near its start")
}
do {
    let short = CGRect(x: 500, y: 600, width: 40, height: 18)
    let p = instructionPopoverPlacement(anchor: short, body: body, visible: visible)
    expect(p.frame.minX + p.arrowX == short.midX, "short selection: arrow at its middle")
}
do {
    let low = CGRect(x: 300, y: 60, width: 200, height: 20)
    let p = instructionPopoverPlacement(anchor: low, body: body, visible: visible)
    expect(p.edge == .bottom, "no room below: flips above")
    expect(p.frame.minY == low.maxY + InstructionArrow.gap, "flipped arrow tip just above the selection")
}
do {
    let right = CGRect(x: 1400, y: 500, width: 30, height: 18)
    let p = instructionPopoverPlacement(anchor: right, body: body, visible: visible)
    expect(p.frame.maxX == visible.maxX - InstructionArrow.margin, "clamped to the right edge")
    expect(p.arrowX == p.frame.width - InstructionArrow.edgeInset, "arrow slides right but stays clear of the corner")
    let nearRight = CGRect(x: 1250, y: 500, width: 30, height: 18)
    let q = instructionPopoverPlacement(anchor: nearRight, body: body, visible: visible)
    expect(q.frame.minX + q.arrowX == nearRight.midX, "arrow still points at the selection after clamping")
}
do {
    let left = CGRect(x: -100, y: 500, width: 20, height: 18)
    let p = instructionPopoverPlacement(anchor: left, body: body, visible: visible)
    expect(p.frame.minX == visible.minX + InstructionArrow.margin, "clamped to the left edge")
    expect(p.arrowX == InstructionArrow.edgeInset, "arrow pinned to the minimum inset")
}
do {
    let tall = CGRect(x: 200, y: 20, width: 600, height: 840)
    let p = instructionPopoverPlacement(anchor: tall, body: body, visible: visible)
    expect(p.frame.minY >= visible.minY + InstructionArrow.margin && p.frame.maxY <= visible.maxY - InstructionArrow.margin,
           "a selection taller than the screen keeps the popover on screen")
}
do {
    let pointer = CGRect(x: 700, y: 400, width: 0, height: 0)
    let p = instructionPopoverPlacement(anchor: pointer, body: body, visible: visible)
    expect(p.edge == .top && p.frame.maxY == 400 - 10 - InstructionArrow.gap, "pointer fallback clears the I-beam")
    expect(p.frame.minX + p.arrowX == 700, "pointer fallback points at the pointer")
}
do {
    let second = CGRect(x: 1440, y: -200, width: 1920, height: 1055)
    let selection = CGRect(x: 1600, y: 300, width: 100, height: 18)
    let p = instructionPopoverPlacement(anchor: selection, body: body, visible: second)
    expect(second.contains(p.frame), "secondary display frames work in global coordinates")
}
do {
    let path = instructionBubblePath(size: CGSize(width: 470, height: h), edge: .top, arrowX: 64)
    expect(path.contains(CGPoint(x: 64, y: h - 1.5)), "top arrow tip is inside the outline")
    expect(!path.contains(CGPoint(x: 200, y: h - 1)), "outside the arrow, the top strip is clear")
    let flipped = instructionBubblePath(size: CGSize(width: 470, height: h), edge: .bottom, arrowX: 64)
    expect(flipped.contains(CGPoint(x: 64, y: 1.5)), "bottom arrow tip is inside the outline")
}

// MARK: Event encoding (decoded by Rust's instruction::decode_event)

expect(InstructionEvent.submit("Make it shorter.\nKeep names").encoded == "submit\nMake it shorter.\nKeep names", "submit")
expect(InstructionEvent.save("x").encoded == "save\nx", "save")
expect(InstructionEvent.cancel.encoded == "cancel" && InstructionEvent.dismiss.encoded == "dismiss", "close events")

// MARK: Controller behavior

_ = NSApplication.shared
NSApp.setActivationPolicy(.accessory)
let controller = InstructionPopoverController()
var wakes = 0
controller.wake = { wakes += 1 }
controller.present(app: "Mail", chars: 129, history: ["Translate to Spanish", "Make it formal"],
                   anchor: CGRect(x: 300, y: 500, width: 300, height: 18))
expect(controller.panel.isVisible, "present shows the panel")
controller.toggle(.shorter)
controller.toggle(.warmer)
expect(controller.instruction == "Make it shorter and warmer.", "chips fill the field")
controller.setDetail("Keep the greeting.")
expect(controller.instruction == "Make it shorter and warmer. Keep the greeting.", "detail follows the sentence")
controller.toggle(.warmer)
expect(controller.instruction == "Make it shorter. Keep the greeting.", "chip toggles keep the typed detail")
controller.save()
expect(controller.takeEvent() == .save("Make it shorter. Keep the greeting."), "⌘S queues save and stays open")
expect(controller.panel.isVisible, "save leaves the popover open")
controller.setDetail("")
controller.toggle(.shorter)
controller.recall(1)
expect(controller.instruction == "Translate to Spanish" && controller.historyCursor == 0, "↑ recalls the newest")
controller.recall(1)
expect(controller.instruction == "Make it formal", "↑ again walks older")
controller.recall(-1)
controller.recall(-1)
expect(controller.instruction == "" && controller.historyCursor == nil, "↓ returns to the empty field")
controller.toggle(.bullets)
controller.submit()
expect(!controller.panel.isVisible, "submit hides before Rust pastes")
expect(controller.takeEvent() == .submit("Rewrite it as bullet points."), "submit queues the instruction")
expect(controller.takeEvent() == nil, "queue drained")
controller.submit()
expect(controller.takeEvent() == nil, "no double submit after closing")
controller.present(app: nil, chars: 3, history: [], anchor: nil)
controller.submit()
expect(controller.takeEvent() == nil && controller.panel.isVisible, "empty instruction does not submit")
controller.cancel()
expect(controller.takeEvent() == .cancel && !controller.panel.isVisible, "Esc queues cancel and hides")
expect(wakes == 3, "every event wakes Rust (\(wakes))")

if failures > 0 {
    fputs("\(failures) instruction test(s) failed\n", stderr)
    exit(1)
}
print("instruction popover: all tests passed")
