# Native command progress preview

Selected design: a fully transparent 48 × 48 point passive panel with no
visible text. It renders the `ThinkingOrb` `.working` state from the `.px64`
preset at 40 points, with a subtle contrast halo. Hover swaps the orb for a
plain `xmark` cancel icon in the same location; the button keeps its tooltip
and accessibility label, and Escape remains available. Verified success
briefly shows a green check in that same slot.

Run `sh examples/run-native-progress-preview.sh` from the repository root.
Swift Package Manager builds the preview and its local library dependency.

The panel is a clear, borderless, nonactivating `NSPanel` backed by an
`NSHostingView` for the MIT-licensed [Thinking Orbs](https://libraries.dev/orbs)
SwiftUI library. Its `.working` animation uses the tuned `.px64` preset while
the preview's Reduce Motion control, plus system Reduce Motion, freezes it.
Replay runs a simulated rewrite in its own draft. Hovering the panel exposes
the cancel button; Cancel or Escape stops that simulation. Dark background and
Reduce Motion controls let you compare the presentation. The orb keeps the
production artwork on both backgrounds: dark particles with a white halo.

Production placement prefers the editor gutter beside the selected text or
caret, keeping the indicator clear of the selection and surrounding toolbar.
When selection range bounds are unavailable, it keeps that editor-gutter
placement and uses the saved pointer offset captured when the command starts
for the vertical position. Only when no editor gutter is available does it
fall back to a diagonal offset from that saved pointer, then keeps the panel
within the visible display. For this demo, the orb is anchored in the gutter
immediately left of the sample draft paragraph.

Integration preserves the existing progress and cancellation interface.
Animate the orb while waiting or streaming, then show the brief green check
only after verified replacement. Unverified replacement continues to save
quietly to History. Reuse the existing global Escape handler; the demo's
Escape monitor is local to its preview window.

Appearance timings: 180 ms fade in, working orb, 650 ms verified-success
acknowledgment, 200 ms fade out. Very fast commands should skip the indicator
to avoid a flash. Production delayed show/hide callbacks must be scoped to a
command generation so an old completion cannot hide a new command's progress.

The MIT-licensed ThinkingOrbsKit source is vendored for a reproducible local
build because the upstream Swift package lives in a repository subdirectory.
See `../apps/selara/native/ThinkingOrbsKit/UPSTREAM.md` for the exact revision
and provenance. No web view, npm packages, or runtime network request is
needed.
