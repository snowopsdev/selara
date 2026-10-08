# selara-ux-review — data manifest

Fixtures: `apps/selara-desktop/dev/mock-tauri.js` (`configured` / `fresh`), structs in
`crates/selara-core/src/{history,usage}.rs`. Counts are from the mock dataset.

| Element | Field path | Class | Notes |
|---|---|---|---|
| Moment-of-use text (1A–D) | `history[1].original` / `.result` (Concise) | ✅ Real | 129 → 61 chars |
| Command name / model in capsule (1B, 1D) | `command.label`, `command.model ?? provider.model` | ✅ Real | |
| Elapsed time (1B) | run start → finish in `serve` | ⚠️ Derive | measured at runtime, not stored |
| Selection bounds for sweep/anchor (1A, 2B, 1D) | AX `kAXBoundsForRangeParameterizedAttribute` | ⚠️ Sometimes | missing in some Electron/canvas apps → orb fallback |
| Streaming partial text (1D) | provider stream | ❌ Aspirational | providers return the full response today |
| Spotlight command list + keycaps (2A) | `config.commands[].label/prompt/hotkey` | ✅ Real | 6/6 |
| Recent instructions (2A) | `instruction_history` (in memory, 10) | ✅ Real | not persisted across restarts |
| Secret span highlight / redaction (2+) | secret detector hit ranges | ❌ Aspirational | detector returns hit kinds, not ranges |
| Size meter thresholds (2+, 8A) | `config.limits.*` | ✅ Real | 4000 / 8000 / 100000 |
| Token/cost estimate before send (2+) | price table × chars/4 | ⚠️ Derive | |
| Readiness pips (3A/B) | `serve_status`, accessibility trust, provider config, hotkey count | ✅ Real | |
| Try-it result (3C, 4A) | new `run_prompt` Tauri command | ❌ Aspirational | CLI `selara run` exists |
| Command glyph + color (4B, 6A) | `command.glyph/color` | ❌ Aspirational | new config fields |
| Prompt blocks (4C) | compiled into `command.prompt` | ✅ Real | blocks are UI-only |
| Provider status line (5A) | CLI version, Codex account, keychain state | ✅ Real | |
| Median latency sparkline (5A) | per-request duration | ❌ Aspirational | not in `UsageEvent` |
| “This month” on provider (5A) | `usage.last_30_days` filtered by kind | ⚠️ Derive | |
| Per-command routing (5B) | `command.provider` | ❌ Aspirational | schema change |
| History diff / outcome / app (6A/B) | `HistoryEntry.original/result/outcome/app/ts` | ✅ Real | 4/4 entries |
| Usage tiles (7A) | `usage.today/last_30_days/all_time` | ✅ Real | |
| Daily bars (7A) | `UsageEvent.ts` bucketed by local day | ⚠️ Derive | needs new aggregation in `usage_summary`; bars in the mock are synthetic |
| Provider share (7A) | `usage.models[]` | ✅ Real | 861 / 128 / 42 |
| Words polished / time saved (7B) | `all_time.output × 0.75`, `requests × 1 min` | ⚠️ Derive | assumptions printed on the receipt |
| App chips with icons (8A) | `excluded_apps[]` + NSWorkspace icon | ⚠️ Sometimes | name/prefix entries have no single icon |
| Language native names (8A) | `config.language` + locale table | ✅ Real | |
