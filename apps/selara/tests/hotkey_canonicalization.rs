//! `selara-core` cannot call `parse_hotkey`: that lives in `selara-platform`
//! behind the macOS-only `global_hotkey` dependency. So `canonical_hotkey`
//! carries its own copy of the alias table, and the two can drift apart
//! silently — a spec the service registers one way and the pack importer
//! considers different would let an imported command steal a live binding.
//!
//! `apps/selara` depends on both crates, so this is where they can be held
//! together: for every pair of specs, the two must agree on identity.
#![cfg(target_os = "macos")]

use selara_core::commands::canonical_hotkey;
use selara_platform::macos::parse_hotkey;

/// Deliberately includes each alias, reordered modifiers, casing and spacing
/// variants, and specs that must not parse at all.
const SPECS: &[&str] = &[
    "ctrl+shift+p",
    "Shift + Control + P",
    "control+shift+p",
    "ctrl+shift+q",
    "cmd+alt+space",
    "command+option+space",
    "super+opt+space",
    "meta+alt+space",
    "ctrl+space",
    "alt+space",
    "shift+space",
    "cmd+shift+w",
    "ctrl+alt+shift+9",
    "ctrl+enter",
    "ctrl+return",
    "escape",
    "esc",
    "tab",
    "f1",
    "up",
    "ctrl",       // modifiers only: no key
    "shift+ctrl", // modifiers only: no key
    "",           // empty
];

#[test]
fn canonical_hotkey_matches_what_the_hotkey_manager_would_register() {
    for a in SPECS {
        let canon_a = canonical_hotkey(a);
        let parsed_a = parse_hotkey(a).ok();
        assert_eq!(
            canon_a.is_some(),
            parsed_a.is_some(),
            "canonical_hotkey and parse_hotkey disagree on whether {a:?} is a binding \
             (canonical={canon_a:?}, parses={})",
            parsed_a.is_some()
        );

        for b in SPECS {
            let (canon_b, parsed_b) = (canonical_hotkey(b), parse_hotkey(b).ok());
            let (Some(pa), Some(pb)) = (parsed_a.as_ref(), parsed_b.as_ref()) else {
                continue;
            };
            assert_eq!(
                canon_a == canon_b,
                pa.id() == pb.id(),
                "identity mismatch for {a:?} vs {b:?}: canonical says {}, the hotkey \
                 manager says {} (canonical {canon_a:?} / {canon_b:?})",
                if canon_a == canon_b {
                    "same"
                } else {
                    "different"
                },
                if pa.id() == pb.id() {
                    "same"
                } else {
                    "different"
                },
            );
        }
    }
}
