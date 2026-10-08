// Motion tokens shared by the native moment-of-use surfaces. They mirror the
// Settings CSS custom properties in design-explorations/implementation-contract.md
// (`--motion-*`, `--ease-*`) so the service and Settings move the same way.
import AppKit
import QuartzCore

enum Motion {
    /// Sidebar selection, page switch.
    static let instant: TimeInterval = 0
    /// Toggles, chips, commit checks; also the Reduce Motion crossfade.
    static let micro: TimeInterval = 0.12
    /// Sheets, popovers, the review card.
    static let panel: TimeInterval = 0.24
    /// Growth and the moment of use (afterglow wipe).
    static let spring: TimeInterval = 0.42
    /// Per word / per line reveal offset.
    static let stagger: TimeInterval = 0.038
    /// Success receipts and the afterglow.
    static let hold: TimeInterval = 1.8

    /// Delay before any progress appears, so fast commands never flash.
    static let fastCommandDelay: TimeInterval = 0.18
    /// Ink Sweep specular band, one pass.
    static let shimmerLoop: TimeInterval = 1.3
    /// Review card skeleton bars, one pass.
    static let skeletonLoop: TimeInterval = 1.1
    /// Afterglow fade after its wipe.
    static let afterglowFade: TimeInterval = 1.5

    /// `--ease-out: cubic-bezier(.2,.8,.2,1)`
    static var easeOut: CAMediaTimingFunction { CAMediaTimingFunction(controlPoints: 0.2, 0.8, 0.2, 1) }
    /// `--ease-spring: cubic-bezier(.2,.9,.25,1.15)`
    static var easeSpring: CAMediaTimingFunction { CAMediaTimingFunction(controlPoints: 0.2, 0.9, 0.25, 1.15) }

    /// Reduce Motion collapses every duration to a `micro` opacity crossfade
    /// and stops loops. Tests and the preview can force it on.
    static var forceReduceMotion = false
    static var reduceMotion: Bool {
        forceReduceMotion || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    }

    /// The duration to use for a transition under the current motion setting.
    static func duration(_ full: TimeInterval) -> TimeInterval {
        reduceMotion ? micro : full
    }
}
