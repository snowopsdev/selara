# Selara UX implementation contract

Source of truth for the parallel implementation of the directions chosen from
`design-explorations/selara-ux-review.html` (open it in a browser: every
variant there is the visual reference). Chosen: **1A+1D, 2B, 3A+3C, 4A+4B, 5A,
6A, 7A+7B, 8A, 9**. Data gaps and decisions: `selara-ux-review.manifest.md`.

Toolchain: use `cargo +1.95.0` (the default stable here is 1.94 and egui needs
1.95). `selara-desktop` needs its sidecar first: `cd apps/selara-desktop &&
npm ci && npm run prepare-sidecar` (or check only `-p selara-core -p selara`).

## Decisions made with the owner

| Gap | Decision |
|---|---|
| 5A median latency | **Record it**: `UsageEvent.duration_ms`, timed from request send to complete reply. Providers shows a designed “No timing yet” state until data exists. |
| 1D streaming | **No streaming**: skeleton shimmer → full word diff. ↩ replace, ⇥ another take (re-run), ⌘C copy, Esc discard. |
| 7B share widget | **1080×1350 portrait PNG** with **Copy Image** and **Save Image…**. Aggregates only, never prompt or selection text. |
| 4B glyph/color | New optional config fields; when unset the UI **derives** a monogram (first letter) and a color (stable hash of id → palette). |
| 7A daily series | **Derived** in core from `usage.jsonl` timestamps (local days). |
| 8A app icons | Looked up via NSWorkspace; wildcard/name-only entries get a **monogram tile**. |
| 1A selection bounds | AX per-line bounds; **fall back to the existing orb** when bounds are missing/implausible. |
| 3C / 4A try-it | New Tauri command (below). Counts toward Usage. |
| Delivery | Stacked PRs by area: PR1 core+backend, PR2 native moment of use (1A+1D+2B), PR3 Settings UI. |

## Schema (already on `feat/core-ux-foundations`)

`WritingCommand` (crates/selara-core/src/commands.rs) gained:
- `glyph: Option<String>`: one grapheme, e.g. `"✂︎"`.
- `color: Option<String>`: `#rrggbb`.
- `review: bool` (default false, omitted when false). When true, serve shows the Ghost Diff card (1D) and replaces only on accept.

`UsageEvent.duration_ms: Option<u64>`. `UsageSummary` gained:
- `daily: Vec<DailyUsage { day: "YYYY-MM-DD", requests, cost_usd: Option<f64> }>`: exactly 30 entries, oldest first, ending today (local), empty days included.
- `providers: Vec<ProviderActivity { kind, last_ts, median_ms: Option<u64>, recent_ms: Vec<u64> (≤20, oldest first), last_30_days: UsageBucket }>`: one per usage `kind` label seen.

Usage kind labels ↔ Settings provider kinds: `openai_compatible`↔`open_ai_compatible`,
`openrouter`↔`open_router`, `anthropic`↔`anthropic`, `chatgpt_codex`↔`codex`,
CLI providers use their own labels (`claude_cli`, `cursor_cli`, `open_code_cli`).
Check `usage.rs` `KIND_*` constants and `cli_provider.rs` for the exact strings.

## New Tauri commands (PR1 implements; mock bridge implements the same shapes)

| Command | Args | Returns |
|---|---|---|
| `try_command` | `{ prompt: string, text: string, model?: string \| null }` | `{ result: string, elapsed_ms: number, model: string, provider: string }`: runs with the active provider (model override if given) through the same cleanup as a real run; records usage. Errors are user-readable strings. |
| `app_icon` | `{ app: string }` (name or bundle id; `*` globs return null) | `string \| null`: `data:image/png;base64,…` 64×64 |
| `running_apps` | none | `[{ name: string, bundle_id: string }]`: regular (Dock) apps, sorted by name, excluding Selara |
| `choose_app` | none | `{ name, bundle_id } \| null`: native open panel rooted at /Applications for `.app` bundles |
| `copy_png` | `{ png_base64: string }` | `null`: puts the image on the general pasteboard |
| `save_png` | `{ png_base64: string, suggested_name: string }` | `string \| null`: save panel; returns the saved path, null if cancelled |

Existing ids and `invoke` names in `index.html` stay load-bearing (see CLAUDE.md).
`#command-sheet` and its field ids may be re-hosted in the inline editor but must keep
their ids, or the e2e tests must be updated in the same PR with the reason.

## Motion tokens (9): CSS custom properties, owned by Settings UI stream A

```css
--motion-instant: 0ms;                                  /* sidebar selection, page switch */
--motion-micro: 120ms; --ease-out: cubic-bezier(.2,.8,.2,1);  /* toggles, commit check, chips */
--motion-panel: 240ms;                                  /* sheet/popover/capsule */
--motion-spring: 420ms; --ease-spring: cubic-bezier(.2,.9,.25,1.15); /* growth, moment of use */
--motion-stagger: 38ms;                                 /* per word / per bar reveal */
--motion-hold: 1800ms;                                  /* success receipts, afterglow */
```
`@media (prefers-reduced-motion: reduce)`: durations collapse to a 120 ms
opacity crossfade, loops stop, and the shake becomes a red outline only. Swift mirrors
these as `enum Motion` constants in the native stream. Other streams reference
`var(--motion-micro, 120ms)` etc. with fallbacks.

Settings-wide behaviors from 9: field commit shows an inline `✓ Saved` that
fades after `--motion-hold` (the footer stays quiet), invalid commit shakes
(`320ms`), and page switch is an instant swap with a 120 ms content crossfade.

## Stream ownership (avoid conflicts)

| Stream | Owns | Must not edit |
|---|---|---|
| backend | `crates/selara-core/**`, `apps/selara-desktop/src-tauri/**` | `index.html`, `dev/`, `apps/selara/**` |
| native-moment (1A+1D) | `apps/selara/native/Overlay*.swift` (new), `apps/selara/native/Progress.swift`, `apps/selara/src/progress.rs`, review/replace path in `serve.rs`, `examples/native-progress-preview` | instruction phase UI in serve.rs, core, desktop |
| native-instruction (2B) | `apps/selara/native/Instruction*.swift` (new), `apps/selara/src/instruction.rs` (new), `UiPhase::Instruction` handling in `serve.rs` | Progress.swift, replace path |
| settings-a (3A+3C, 4A+4B, 9) | Status + Commands sections, motion tokens, the command editor, matching mock handlers + e2e | Providers/History/Usage/General/Limits render code |
| settings-b (5A, 6A, 7A+7B, 8A) | Providers, History, Usage (incl. receipt), General, Limits sections, matching mock handlers + e2e | Status/Commands render code, motion token block |

Settings streams: do **not** regenerate `dist/` (the merge step does it once). Keep
mock data realistic: extend `dev/mock-tauri.js` fixtures (e.g. `daily`,
`providers`, `duration_ms`) for the `configured` and `fresh` scenarios; `fresh`
must look designed (sparse state), not broken.
