# Thinking Orbs

The native writing progress panel adapts the `working`, 64px preset from
[Jakub Antalik's thinking-orbs v0.3.1](https://github.com/Jakubantalik/thinking-orbs/tree/v0.3.1).
The pure Rust geometry is in `apps/selara/src/progress/orb.rs`; AppKit renders
its dots directly without a web view or JavaScript runtime.

The port retains the orbit geometry, depth ordering, dot styling, and preset
speed. It follows the system appearance and uses a static frame when Reduce
Motion is enabled. The panel's existing working-state refresh drives animation
only while visible.

The upstream MIT license is reproduced in [LICENSE](LICENSE) and copied into
the desktop app's runtime notices and the CLI release archive.
