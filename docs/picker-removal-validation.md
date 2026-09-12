# Picker removal validation

Initial validation: 2026-09-11. Automated checks rerun for PR preparation: 2026-09-12. Platform: Apple Silicon macOS. Rust: 1.95.0.

Implementation and automated verification are complete. Release remains blocked on the live editor matrix below.

## Automated checks

| Check | Result |
| --- | --- |
| `rustup run 1.95.0 cargo test --workspace` | Passed: 253 tests; 2 existing tests ignored (native runtime handshake and live OpenRouter model listing) |
| `rustup run 1.95.0 cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `rustup run 1.95.0 cargo fmt --all -- --check` | Passed |
| `npm test` in `apps/selara-desktop` | Passed: 51 tests, including command editing and Models regressions |
| `npm run build` in `apps/selara-desktop` | Passed; committed `dist` rebuilt |
| `rustup run 1.95.0 cargo build -p selara-desktop --features tauri/custom-protocol` | Passed with embedded frontend and real local sidecars |
| CLI stdin/stdout against an isolated local provider | Passed for Proofread, Proofread with `--no-stream`, and a legacy popup Summary command with `--no-stream` |
| `git diff --check` | Passed |

Regression coverage includes schema and command-pack normalization, historical entries, formatting-preserving output cleaning, reserved shortcuts, protocol framing and run IDs, stale completion/cancellation decisions, quiescing, app filters, instruction recall and saving, warning thresholds, empty input, UTF-16 ranges, clipboard ownership and guarded restoration, concurrent config updates, and lock recovery after a killed writer.

Independent read-only review covered the final selection, clipboard, hotkey, protocol, service lifecycle, and migration changes. Final PR review caught and verified a fix for menu dispatch stripping repeated `command:` prefixes from configured IDs; a regression now preserves those IDs exactly. No actionable P1/P2 findings remained. This review does not substitute for live editor testing.

## Live observations

- In TextEdit, a completed command replaced `Before.  I recieve teh café report.  After.` with `Before.  I receive the café report.  After.` through native paste. One native Command-Z restored the exact original text, including the accented character and doubled spaces.
- Later attempts with an isolated service could not reliably activate TextEdit through the automation interface: the service reported its own PID as frontmost. Admission rejected the unverified source and left the document unchanged. This prevents treating those attempts as successful cancellation or focus-switch tests.
- Settings was inspected at 920 × 640 and its minimum 760 × 520 using the actual HTML and synthetic backend responses. Commands, the command editor, and Models were checked in both appearances; General was also captured. The command editor keeps failed drafts open, contains keyboard focus, and keeps actions visible. Models preserves provider drafts across mode switches and account refreshes, keeps Save visible, and supports retry after a failed save. Screenshots were refreshed. These are frontend previews, not evidence of native tray execution or real account sign-in.
- Preview navigation logged a MutationObserver error that also reproduced in an isolated, script-free iframe. No Selara application error was reproduced in the exercised Settings flows. Native WebKit and VoiceOver still need verification.
- Tests used synthetic text, a localhost provider, and a temporary config. The installed application and normal user config were not changed.

## Required before release

Complete this matrix using the final packaged application. Record the app/version, command entry point, result, and native undo result for each case.

| Area | Required live evidence |
| --- | --- |
| Editors | TextEdit, Notes, a browser editor, and an Electron editor; menu and direct shortcut execution; one Command-Z restores exact Unicode, whitespace, and surrounding rich-text formatting |
| Source identity | Multiple source windows, Settings already open, a moved selection, a changed document, closed controls/windows, and focus changes during generation |
| Cancellation | Progress-panel Cancel, Escape, delayed responses after cancellation, stale cancellation after a newer run, and cancellation across service restarts |
| Clipboard | A copy during generation, a copy immediately before paste, a copy before restoration, existing multiple clipboard formats, and slow paste consumption |
| Dialogs | Empty selection, hard limit, soft and replacement warnings, suspected-secret confirmation, custom instruction recall, and Save as command |
| Service | Menu refresh after config edits, external/incompatible service explanation, quiescing during a slow request, and recovery after a managed service restart |
| Verification failure | Failed or uncertain paste is never automatically repeated and never recorded as successful history |

Native undo behavior belongs to the target editor. Do not infer compatibility from a successful build, unit tests, or the single TextEdit observation.
